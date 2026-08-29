local Controller = require("aem.controller")

local controller = nil

function init(plugin)
  controller = Controller.new {
    app = app,
    plugin = plugin,
    Dialog = Dialog,
    WebSocket = WebSocket,
    WebSocketMessageType = WebSocketMessageType,
    Timer = Timer,
    json = json,
    io = io,
    os = os,
  }

  plugin:newCommand {
    id = "AsepriteExtensionManager",
    title = "Aseprite Extension Manager…",
    group = "file_scripts",
    onclick = function()
      controller:open()
    end,
    onenabled = function()
      return app.isUIAvailable
    end,
  }

  controller:schedule_startup_check()
end

function exit(plugin)
  if controller then
    controller:close()
    controller = nil
  end
end
