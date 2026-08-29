local Test = require("testlib")
local JsonValue = require("aem.json_value")
local Launcher = require("aem.launcher")
local Rpc = require("aem.rpc")

Test.case("decoded value normalization rejects cycles and excessive depth", function()
  local cycle = {}
  cycle.self = cycle
  local normalized, cycle_error = JsonValue.normalize(cycle)
  Test.equal(normalized, nil)
  Test.contains(cycle_error, "cycle")

  local nested = {
    child = {
      child = {
        value = true,
      },
    },
  }
  local depth_value, depth_error = JsonValue.normalize(nested, {
    maxDepth = 1,
  })
  Test.equal(depth_value, nil)
  Test.contains(depth_error, "deeply")
end)

Test.case("decoded value normalization enforces node and string limits", function()
  local node_value, node_error = JsonValue.normalize({
    1,
    2,
    3,
  }, {
    maxNodes = 3,
  })
  Test.equal(node_value, nil)
  Test.contains(node_error, "too many")

  local string_value, string_error = JsonValue.normalize({
    value = "12345",
  }, {
    maxStringBytes = 4,
  })
  Test.equal(string_value, nil)
  Test.contains(string_error, "oversized")
end)

if json and type(json.decode) == "function" then
  Test.case("Aseprite JSON objects and nested arrays normalize to plain tables", function()
    local decoded = json.decode([[
      {
        "protocol": 1,
        "nullable": null,
        "result": {
          "packages": [
            {"name": "one", "versions": ["1.0.0", "1.1.0"]},
            {"name": "two", "enabled": true}
          ],
          "emptyObject": {},
          "emptyArray": []
        }
      }
    ]])
    Test.equal(type(decoded), "userdata")

    local normalized, normalize_error = JsonValue.normalize(decoded)
    Test.truthy(normalized, normalize_error)
    Test.equal(type(normalized), "table")
    Test.equal(normalized.nullable, nil)
    Test.equal(type(normalized.result), "table")
    Test.equal(type(normalized.result.packages), "table")
    Test.equal(type(normalized.result.packages[1]), "table")
    Test.equal(normalized.result.packages[1].name, "one")
    Test.equal(normalized.result.packages[1].versions[2], "1.1.0")
    Test.equal(normalized.result.packages[2].enabled, true)
    Test.equal(type(normalized.result.emptyObject), "table")
    Test.equal(type(normalized.result.emptyArray), "table")
  end)

  Test.case("Aseprite JSON null array entries are rejected without truncation", function()
    local decoded = json.decode([[{"values":[1,null,3]}]])
    local normalized, normalize_error = JsonValue.normalize(decoded)
    Test.equal(normalized, nil)
    Test.contains(normalize_error, "null")
  end)

  Test.case("launcher accepts Aseprite native decoded handshake objects", function()
    local token = string.rep("A", 43)
    local handshake, handshake_error = Launcher.parse_handshake(
      '{"protocol":1,"port":43123,"token":"'
        .. token
        .. '","path":"/v1/'
        .. token
        .. '","pid":42,"version":"0.1.0"}',
      json
    )
    Test.truthy(handshake, handshake_error)
    Test.equal(handshake.port, 43123)
    Test.equal(handshake.token, token)
  end)

  Test.case("RPC callbacks receive plain tables from Aseprite JSON userdata", function()
    local result
    local rpc = Rpc.new {
      json = json,
    }
    rpc.pending.request = {
      id = "request",
      cancelled = false,
      onSuccess = function(value)
        result = value
      end,
    }

    rpc:_handle_text([[
      {
        "protocol": 1,
        "id": "request",
        "ok": true,
        "result": {
          "packages": [
            {"name": "example", "update": {"version": "2.0.0"}}
          ]
        }
      }
    ]])

    Test.equal(type(result), "table")
    Test.equal(type(result.packages), "table")
    Test.equal(type(result.packages[1]), "table")
    Test.equal(type(result.packages[1].update), "table")
    Test.equal(result.packages[1].update.version, "2.0.0")
  end)
end
