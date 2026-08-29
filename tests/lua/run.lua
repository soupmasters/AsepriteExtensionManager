local root
if app and app.params and app.params.root then
  root = app.params.root
elseif arg and arg[1] then
  root = arg[1]
else
  root = "."
end

local separator = package.config:sub(1, 1)
package.path = table.concat({
  root .. separator .. "extension" .. separator .. "?.lua",
  root .. separator .. "tests" .. separator .. "lua" .. separator .. "?.lua",
  package.path,
}, ";")

require("compat_platform_test")
require("json_value_test")
require("launcher_rpc_test")
require("model_ui_test")
require("controller_test")

require("testlib").run()
