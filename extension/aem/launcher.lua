local Platform = require("aem.platform")
local Protocol = require("aem.protocol")
local JsonValue = require("aem.json_value")

local Launcher = {}

local function valid_token(token)
  if type(token) ~= "string" then
    return false
  end
  local base64url = #token == 43 and token:match("^[A-Za-z0-9_-]+$") ~= nil
  local hexadecimal = #token == 64 and token:match("^%x+$") ~= nil
  return base64url or hexadecimal
end

function Launcher.command(environment)
  local app = environment.app
  local plugin = environment.plugin
  local helper_path, path_error = Platform.helper_path(app, plugin.path)
  if not helper_path then
    return nil, path_error
  end
  if not app.fs.isFile(helper_path) then
    return nil, "The bundled helper is missing for " .. tostring(app.os.name or "this platform") .. "."
  end

  local platform_key = Platform.key(app.os)
  local values = {
    helper_path,
    "launch",
    "--user-config",
    app.fs.userConfigPath,
    "--extension-root",
    plugin.path,
  }
  local quoted = {}
  for index, value in ipairs(values) do
    local item, quote_error = Platform.shell_quote(value, platform_key)
    if not item then
      return nil, quote_error
    end
    quoted[index] = item
  end

  return table.concat(quoted, " "), helper_path
end

function Launcher.parse_handshake(line, json_api)
  if type(line) ~= "string" or line == "" or #line > 4096 then
    return nil, "The helper returned an invalid launch response."
  end

  local decoded_ok, decoded = pcall(json_api.decode, line)
  if not decoded_ok then
    return nil, "The helper returned malformed launch data."
  end
  local handshake = JsonValue.normalize(decoded, {
    maxDepth = 12,
    maxNodes = 256,
    maxStringBytes = 4096,
    maxTotalStringBytes = 4096,
    maxSerializedBytes = 4096,
  })
  if type(handshake) ~= "table" then
    return nil, "The helper returned malformed launch data."
  end
  if handshake.protocol ~= Protocol.VERSION then
    return nil, "The helper uses an incompatible protocol version."
  end

  local port = tonumber(handshake.port)
  if not port or port ~= math.floor(port) or port < 1 or port > 65535 then
    return nil, "The helper returned an invalid loopback port."
  end
  if not valid_token(handshake.token) then
    return nil, "The helper returned an invalid session token."
  end

  local expected_path = "/v1/" .. handshake.token
  if handshake.path ~= expected_path then
    return nil, "The helper returned an invalid session path."
  end

  return {
    protocol = Protocol.VERSION,
    port = port,
    token = handshake.token,
    path = expected_path,
    pid = handshake.pid,
    version = handshake.version,
    url = "ws://127.0.0.1:" .. tostring(port) .. expected_path,
  }
end

function Launcher.launch(environment)
  local command, command_error = Launcher.command(environment)
  if not command then
    return nil, command_error
  end

  local open_ok, pipe, pipe_error = pcall(environment.io.popen, command, "r")
  if not open_ok or not pipe then
    return nil, tostring(pipe_error or pipe or "Aseprite did not allow the helper to start.")
  end

  local read_ok, line = pcall(function()
    return pipe:read("*l")
  end)
  local close_ok, exited, exit_kind, exit_code = pcall(function()
    return pipe:close()
  end)

  if not read_ok then
    return nil, "The helper launcher could not be read."
  end
  if not close_ok then
    return nil, "The helper launcher did not exit cleanly."
  end
  if exited == nil or exited == false or (exit_kind == "exit" and tonumber(exit_code or 0) ~= 0) then
    return nil, "The helper launcher reported an error."
  end

  return Launcher.parse_handshake(line, environment.json)
end

return Launcher
