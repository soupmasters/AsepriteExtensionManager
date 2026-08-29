local Launcher = require("aem.launcher")
local Protocol = require("aem.protocol")
local JsonValue = require("aem.json_value")

local Rpc = {}
Rpc.__index = Rpc

local MAX_MESSAGE_BYTES = 8 * 1024 * 1024

local function structured_error(code, message, retryable)
  return {
    code = code,
    message = message,
    retryable = retryable == true,
  }
end

function Rpc.new(environment)
  return setmetatable({
    environment = environment,
    state = "closed",
    socket = nil,
    handshake = nil,
    pending = {},
    nextId = 0,
    connectionTimer = nil,
    intentionalClose = false,
  }, Rpc)
end

function Rpc:_new_id()
  self.nextId = self.nextId + 1
  local now = self.environment.os and self.environment.os.time and self.environment.os.time() or 0
  return "aem-" .. tostring(now) .. "-" .. tostring(self.nextId)
end

function Rpc:_stop_connection_timer()
  if self.connectionTimer then
    pcall(function()
      self.connectionTimer:stop()
    end)
    self.connectionTimer = nil
  end
end

function Rpc:_fail_pending(error_value)
  local pending = self.pending
  self.pending = {}
  for _, request in pairs(pending) do
    if not request.cancelled and request.onError then
      pcall(request.onError, error_value)
    end
  end
end

function Rpc:_close_socket()
  self:_stop_connection_timer()
  local socket = self.socket
  self.socket = nil
  self.state = "closed"
  self.handshake = nil
  if socket then
    pcall(function()
      socket:close()
    end)
  end
end

function Rpc:_send(request)
  if request.cancelled or request.sent then
    return
  end

  local encode_ok, payload = pcall(self.environment.json.encode, {
    protocol = Protocol.VERSION,
    id = request.id,
    method = request.method,
    params = request.params,
  })
  if not encode_ok or type(payload) ~= "string" then
    self.pending[request.id] = nil
    if request.onError then
      request.onError(structured_error("encode_failed", "The helper request could not be encoded."))
    end
    return
  end

  local send_ok, send_error = pcall(function()
    self.socket:sendText(payload)
  end)
  if not send_ok then
    self.pending[request.id] = nil
    if request.onError then
      request.onError(structured_error("send_failed", tostring(send_error), true))
    end
    return
  end
  request.sent = true
end

function Rpc:_flush()
  for _, request in pairs(self.pending) do
    self:_send(request)
  end
end

function Rpc:_handle_progress(message)
  if type(message.operationId) ~= "string" then
    return
  end
  local request = self.pending[message.operationId]
  if request and not request.cancelled and request.onProgress then
    pcall(request.onProgress, message)
  end
end

function Rpc:_handle_text(data)
  if type(data) ~= "string" or #data > MAX_MESSAGE_BYTES then
    self:_fail_pending(structured_error("invalid_message", "The helper returned an invalid message."))
    self:_close_socket()
    return
  end

  local decode_ok, decoded = pcall(self.environment.json.decode, data)
  local message
  if decode_ok then
    message = JsonValue.normalize(decoded, {
      maxDepth = 32,
      maxNodes = 50000,
      maxStringBytes = 1024 * 1024,
      maxTotalStringBytes = MAX_MESSAGE_BYTES,
      maxSerializedBytes = MAX_MESSAGE_BYTES,
    })
  end
  if type(message) ~= "table" or message.protocol ~= Protocol.VERSION then
    self:_fail_pending(structured_error("protocol_error", "The helper returned an incompatible message."))
    self:_close_socket()
    return
  end

  if message.event == "progress" then
    self:_handle_progress(message)
    return
  end

  if type(message.id) ~= "string" then
    return
  end
  local request = self.pending[message.id]
  if not request then
    return
  end
  self.pending[message.id] = nil
  if request.cancelled then
    return
  end

  if message.ok == true then
    if request.onSuccess then
      pcall(request.onSuccess, message.result or {})
    end
  elseif request.onError then
    local error_value = message.error
    if type(error_value) ~= "table" then
      error_value = structured_error("helper_error", "The helper returned an error.")
    end
    pcall(request.onError, error_value)
  end
end

function Rpc:_handle_socket_message(message_type, data, socket_error)
  local types = self.environment.WebSocketMessageType
  if message_type == types.OPEN then
    self.state = "open"
    self:_stop_connection_timer()
    self:_flush()
  elseif message_type == types.TEXT then
    self:_handle_text(data)
  elseif message_type == types.CLOSE then
    local was_intentional = self.intentionalClose
    self.intentionalClose = false
    self:_close_socket()
    if not was_intentional then
      self:_fail_pending(structured_error(
        "connection_closed",
        tostring(socket_error or data or "The helper connection closed."),
        true
      ))
    end
  end
end

function Rpc:connect()
  if self.state == "open" or self.state == "connecting" then
    return true
  end

  local handshake, launch_error = Launcher.launch(self.environment)
  if not handshake then
    return false, launch_error
  end
  self.handshake = handshake
  self.state = "connecting"

  local create_ok, socket = pcall(self.environment.WebSocket, {
    url = handshake.url,
    deflate = false,
    minreconnectwait = 0.25,
    maxreconnectwait = 1.0,
    onreceive = function(message_type, data, socket_error)
      self:_handle_socket_message(message_type, data, socket_error)
    end,
  })
  if not create_ok or not socket then
    self.state = "closed"
    self.handshake = nil
    return false, tostring(socket or "The localhost WebSocket could not be created.")
  end
  self.socket = socket

  if self.environment.Timer then
    local timer
    timer = self.environment.Timer {
      interval = 10,
      ontick = function()
        timer:stop()
        if self.state == "connecting" then
          self:_fail_pending(structured_error(
            "connection_timeout",
            "The helper did not accept the localhost connection.",
            true
          ))
          self:_close_socket()
        end
      end,
    }
    self.connectionTimer = timer
    timer:start()
  end

  local connect_ok, connect_error = pcall(function()
    socket:connect()
  end)
  if not connect_ok then
    self:_close_socket()
    return false, tostring(connect_error)
  end
  return true
end

function Rpc:request(method, params, callbacks)
  callbacks = callbacks or {}
  if not Protocol.is_method_allowed(method) then
    if callbacks.onError then
      callbacks.onError(structured_error("method_not_allowed", "The requested helper operation is not allowed."))
    end
    return {
      cancel = function() end,
    }
  end

  local request = {
    id = self:_new_id(),
    method = method,
    params = params or {},
    onSuccess = callbacks.onSuccess,
    onError = callbacks.onError,
    onProgress = callbacks.onProgress,
    sent = false,
    cancelled = false,
  }
  self.pending[request.id] = request

  local connected, connection_error = self:connect()
  if not connected then
    self.pending[request.id] = nil
    if request.onError then
      request.onError(structured_error("launch_failed", connection_error, true))
    end
  elseif self.state == "open" then
    self:_send(request)
  end

  local ticket = {}
  function ticket.cancel()
    if request.cancelled then
      return
    end
    request.cancelled = true
    self.pending[request.id] = nil
  end
  return ticket
end

function Rpc:shutdown(on_done)
  if self.state == "closed" then
    if on_done then
      on_done()
    end
    return
  end

  self:request("shutdown", {}, {
    onSuccess = function()
      self.intentionalClose = true
      self:_close_socket()
      if on_done then
        on_done()
      end
    end,
    onError = function()
      self.intentionalClose = true
      self:_close_socket()
      if on_done then
        on_done()
      end
    end,
  })
end

function Rpc:force_close()
  self.intentionalClose = true
  self:_fail_pending(structured_error("cancelled", "The operation was cancelled."))
  self:_close_socket()
end

return Rpc
