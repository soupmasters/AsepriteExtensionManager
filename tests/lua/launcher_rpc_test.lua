local Test = require("testlib")
local Fakes = require("fakes")
local Launcher = require("aem.launcher")
local Rpc = require("aem.rpc")

local TOKEN = string.rep("A", 43)

local function rpc_environment()
  local json_api = Fakes.json()
  json_api.register("HANDSHAKE", {
    protocol = 1,
    port = 43123,
    token = TOKEN,
    path = "/v1/" .. TOKEN,
    pid = 45,
    version = "0.1.0",
  })

  local app = Fakes.app {
    allFilesExist = true,
  }
  local plugin = Fakes.plugin()
  local pipe = {}
  function pipe:read()
    return "HANDSHAKE"
  end
  function pipe:close()
    return true, "exit", 0
  end

  local socket
  local function WebSocket(options)
    socket = {
      options = options,
      sent = {},
      closed = false,
    }
    function socket:connect()
      self.options.onreceive("open", "")
    end
    function socket:sendText(payload)
      self.sent[#self.sent + 1] = payload
    end
    function socket:close()
      self.closed = true
    end
    function socket:emit(kind, payload, socket_error)
      self.options.onreceive(kind, payload, socket_error)
    end
    return socket
  end

  local environment
  environment = {
    app = app,
    plugin = plugin,
    json = json_api,
    io = {
      popen = function(command)
        environment.command = command
        return pipe
      end,
    },
    os = {
      time = function()
        return 100
      end,
    },
    WebSocket = WebSocket,
    WebSocketMessageType = {
      OPEN = "open",
      TEXT = "text",
      CLOSE = "close",
    },
  }
  return environment, json_api, function()
    return socket
  end
end

Test.case("launcher command passes only fixed helper arguments", function()
  local environment = rpc_environment()
  local handshake, launch_error = Launcher.launch(environment)
  Test.truthy(handshake, launch_error)
  Test.contains(environment.command, "aem-helper")
  Test.contains(environment.command, "'launch'")
  Test.contains(environment.command, "'--user-config'")
  Test.contains(environment.command, "'--extension-root'")
  Test.equal(handshake.url, "ws://127.0.0.1:43123/v1/" .. TOKEN)
end)

Test.case("invalid helper handshake tokens are rejected", function()
  local json_api = Fakes.json()
  json_api.register("BAD", {
    protocol = 1,
    port = 4000,
    token = "short",
    path = "/v1/short",
  })
  local handshake, message = Launcher.parse_handshake("BAD", json_api)
  Test.equal(handshake, nil)
  Test.contains(message, "session token")
end)

Test.case("protocol mismatch is rejected before WebSocket connection", function()
  local json_api = Fakes.json()
  json_api.register("BAD", {
    protocol = 2,
    port = 4000,
    token = TOKEN,
    path = "/v1/" .. TOKEN,
  })
  local handshake, message = Launcher.parse_handshake("BAD", json_api)
  Test.equal(handshake, nil)
  Test.contains(message, "protocol")
end)

Test.case("RPC connects only to authenticated loopback path", function()
  local environment, _, socket_value = rpc_environment()
  local rpc = Rpc.new(environment)
  local result
  rpc:request("ping", {}, {
    onSuccess = function(value)
      result = value
    end,
  })
  local socket = socket_value()
  Test.equal(socket.options.url, "ws://127.0.0.1:43123/v1/" .. TOKEN)
  Test.equal(#socket.sent, 1)
  Test.equal(result, nil)
end)

Test.case("concurrent progress events are isolated by request id", function()
  local environment, json_api, socket_value = rpc_environment()
  local rpc = Rpc.new(environment)
  local first_progress = 0
  local second_progress = 0
  rpc:request("scanInstalled", {}, {
    onProgress = function()
      first_progress = first_progress + 1
    end,
  })
  rpc:request("refreshRegistry", {}, {
    onProgress = function()
      second_progress = second_progress + 1
    end,
  })

  local socket = socket_value()
  local first_request = json_api.value(socket.sent[1])
  local second_request = json_api.value(socket.sent[2])
  Test.truthy(first_request.id ~= second_request.id)

  json_api.register("PROGRESS_ONE", {
    protocol = 1,
    event = "progress",
    operationId = first_request.id,
    message = "Scanning",
  })
  socket:emit("text", "PROGRESS_ONE")
  Test.equal(first_progress, 1)
  Test.equal(second_progress, 0)

  json_api.register("PROGRESS_UNKNOWN", {
    protocol = 1,
    event = "progress",
    operationId = "unknown",
    message = "Ignored",
  })
  socket:emit("text", "PROGRESS_UNKNOWN")
  Test.equal(first_progress, 1)
  Test.equal(second_progress, 0)
end)

Test.case("cancelled RPC requests ignore later responses", function()
  local environment, json_api, socket_value = rpc_environment()
  local rpc = Rpc.new(environment)
  local called = false
  local ticket = rpc:request("diagnostics", {}, {
    onSuccess = function()
      called = true
    end,
  })
  local socket = socket_value()
  local request = json_api.value(socket.sent[1])
  ticket.cancel()
  json_api.register("RESPONSE", {
    protocol = 1,
    id = request.id,
    ok = true,
    result = {},
  })
  socket:emit("text", "RESPONSE")
  Test.falsy(called)
end)

Test.case("RPC marks explicit helper rejections as definitive", function()
  local environment, json_api, socket_value = rpc_environment()
  local rpc = Rpc.new(environment)
  local definitive
  local received_error
  rpc:request("scanInstalled", {}, {
    onError = function(error_value, helper_rejected)
      received_error = error_value
      definitive = helper_rejected
    end,
  })
  local socket = socket_value()
  local request = json_api.value(socket.sent[1])
  json_api.register("HELPER_ERROR", {
    protocol = 1,
    id = request.id,
    ok = false,
    error = {
      code = "INVALID_EXTENSION_PATH",
      message = "The extension path is invalid.",
    },
  })

  socket:emit("text", "HELPER_ERROR")

  Test.truthy(definitive)
  Test.equal(received_error.code, "INVALID_EXTENSION_PATH")
end)
