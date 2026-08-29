local command_id = "AemAcceptanceFixture"

function init(plugin)
  if app.isUIAvailable then
    plugin:newCommand {
      id = command_id,
      title = "Extension Manager Acceptance Fixture",
      group = "file_scripts",
      onclick = function()
        app.alert("The acceptance fixture is installed.")
      end
    }
  end
end

function exit(plugin)
end
