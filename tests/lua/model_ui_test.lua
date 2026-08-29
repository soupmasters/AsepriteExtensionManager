local Test = require("testlib")
local Fakes = require("fakes")
local Model = require("aem.model")
local Ui = require("aem.ui")

Test.case("browse model searches and paginates catalog packages", function()
  local model = Model.new(2)
  model:set_catalog({
    {
      name = "alpha-tools",
      displayName = "Alpha Tools",
      author = "A",
    },
    {
      name = "beta-tools",
      displayName = "Beta Tools",
      author = "B",
    },
    {
      name = "gamma-tools",
      displayName = "Gamma Tools",
      author = "C",
    },
  }, "current", false, false)

  local first, page, pages, total = model:page("browse")
  Test.equal(#first, 2)
  Test.equal(page, 1)
  Test.equal(pages, 2)
  Test.equal(total, 3)

  model:move_page("browse", 1)
  local second = model:page("browse")
  Test.equal(#second, 1)
  Test.equal(second[1].name, "gamma-tools")

  model:set_search("browse", "beta")
  local filtered, filtered_page, filtered_pages, filtered_total = model:page("browse")
  Test.equal(#filtered, 1)
  Test.equal(filtered[1].name, "beta-tools")
  Test.equal(filtered_page, 1)
  Test.equal(filtered_pages, 1)
  Test.equal(filtered_total, 1)
end)

Test.case("manager uses compact native install and utility controls", function()
  local app = Fakes.app {
    allFilesExist = true,
  }
  local plugin = Fakes.plugin {
    version = "9.8.7",
  }
  local Dialog, dialogs = Fakes.dialog_factory()
  local model = Model.new(2)
  local environment = {
    app = app,
    plugin = plugin,
    Dialog = Dialog,
  }
  local ui = Ui.new(environment, model)
  local controller = {
    refresh = function() end,
    install_from_github = function() end,
    sync_local_folder = function() end,
    on_dialog_closed = function() end,
  }
  ui:open(controller)

  local manager = dialogs[1]
  Test.truthy(manager.options.resizeable)
  Test.falsy(manager.options.autofit)
  Test.equal(manager.autofit, Align and Align.TOP or 0)
  Test.equal(type(manager.shown), "table")
  Test.equal(manager.shown.wait, false)
  Test.truthy(manager.shown.autoscrollbars)

  local buttons = {}
  local widget_indices = {}
  local same_row_after = {}
  for index, widget in ipairs(manager.widgets) do
    if widget.kind == "button" then
      buttons[widget.definition.text] = true
    end
    if widget.definition.id then
      widget_indices[widget.definition.id] = index
    end
    Test.falsy(widget.kind == "canvas", "manager must not use a canvas")
  end
  for _, call in ipairs(manager.sameRowCalls) do
    local widget = call.after
    if widget and widget.definition.id then
      same_row_after[widget.definition.id] = true
    end
  end
  local install = manager.widgetsById.install
  Test.equal(install.kind, "button")
  Test.equal(install.definition.text, "Install… +")
  Test.truthy(buttons["↻"])
  Test.truthy(buttons["⚙"])
  Test.truthy(buttons["?"])
  Test.falsy(manager.widgetsById.install_github)
  Test.falsy(manager.widgetsById.sync_local)
  Test.falsy(manager.widgetsById.close)

  local help = manager.widgetsById.help
  Test.equal(help.kind, "button")
  Test.equal(help.definition.text, "?")
  Test.falsy(help.definition.focus)
  Test.falsy(help.definition.hexpand)
  help.definition.onclick()
  local help_dialog = dialogs[2]
  Test.equal(help_dialog.options.title, "About Aseprite Extension Manager")
  Test.equal(help_dialog.options.parent, manager)
  Test.equal(help_dialog.widgetsById.help_version.definition.text, "9.8.7")
  Test.equal(
    help_dialog.widgetsById.help_creator.definition.text,
    "Martin Calander · Soupmasters"
  )
  Test.truthy(help_dialog.options.resizeable)
  Test.truthy(help_dialog.shown.autoscrollbars)

  manager.widgetsById.preferences.definition.onclick()
  local preferences_dialog = dialogs[3]
  Test.equal(preferences_dialog.options.title, "Extension Manager Preferences")
  Test.truthy(preferences_dialog.options.resizeable)
  Test.truthy(preferences_dialog.shown.autoscrollbars)
  Test.equal(preferences_dialog.widgetsById.save_preferences.definition.text, "Save")
  Test.equal(preferences_dialog.widgetsById.cancel_preferences.definition.text, "Discard")

  for _, kind in ipairs({ "browse", "installed" }) do
    for index = 1, model.pageSize do
      local row_id = kind .. "_row_" .. tostring(index)
      local details_id = kind .. "_details_" .. tostring(index)
      local row = manager.widgetsById[row_id]
      local details = manager.widgetsById[details_id]
      Test.equal(row.kind, "label")
      Test.equal(details.kind, "button")
      Test.equal(details.definition.text, "Details")
      Test.falsy(details.definition.hexpand)
      Test.truthy(same_row_after[row_id])
      Test.equal(widget_indices[details_id], widget_indices[row_id] + 1)
      Test.equal(manager.widgets[widget_indices[details_id] + 1].kind, "newrow")
      local stacked_details = manager.widgetsById[kind .. "_stacked_details_" .. tostring(index)]
      Test.equal(stacked_details.kind, "button")
      Test.equal(stacked_details.definition.text, "Details")
      Test.falsy(stacked_details.definition.visible)
      local stacked_index = widget_indices[kind .. "_stacked_details_" .. tostring(index)]
      Test.equal(manager.widgets[stacked_index - 1].kind, "newrow")
    end
  end

  for _, kind in ipairs({ "browse", "installed" }) do
    local previous = manager.widgetsById[kind .. "_previous"].definition
    local page = manager.widgetsById[kind .. "_page"].definition
    local next_page = manager.widgetsById[kind .. "_next"].definition
    Test.equal(previous.text, "←")
    Test.equal(page.text, "1 / 1")
    Test.equal(next_page.text, "→")
    Test.falsy(previous.visible)
    Test.falsy(page.visible)
    Test.falsy(next_page.visible)
    Test.truthy(widget_indices[kind .. "_previous"] < widget_indices[kind .. "_page"])
    Test.truthy(widget_indices[kind .. "_page"] < widget_indices[kind .. "_next"])
  end
end)

Test.case("manager package rows switch between wide and narrow layouts", function()
  local app = Fakes.app {
    allFilesExist = true,
  }
  local Dialog, dialogs = Fakes.dialog_factory {
    shrinkSizeHintOnShow = 120,
  }
  local Timer, timers = Fakes.timer_factory()
  local model = Model.new(1)
  model:set_catalog({
    {
      name = "browse-package",
      version = "1.0.0",
    },
  }, "current", false, false)
  model:set_installed({
    {
      name = "installed-package",
      version = "2.0.0",
      managed = true,
    },
  })
  local ui = Ui.new({
    app = app,
    plugin = Fakes.plugin(),
    Dialog = Dialog,
    Timer = Timer,
  }, model)
  ui:open {
    refresh = function() end,
    install_from_github = function() end,
    sync_local_folder = function() end,
    on_dialog_closed = function() end,
  }

  local manager = dialogs[1]
  Test.equal(ui.wideRowMinWidth, 640)
  timers[#timers]:fire()
  for _, kind in ipairs({ "browse", "installed" }) do
    Test.truthy(manager.widgetsById[kind .. "_details_1"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_stacked_details_1"].definition.visible)
  end

  manager:resize(520)
  local narrow_timer = timers[#timers]
  Test.truthy(narrow_timer.started)
  narrow_timer:fire()
  for _, kind in ipairs({ "browse", "installed" }) do
    Test.falsy(manager.widgetsById[kind .. "_details_1"].definition.visible)
    Test.truthy(manager.widgetsById[kind .. "_stacked_details_1"].definition.visible)
  end
  Test.equal(manager.widgetsById.browse_row_1.definition.text, "browse-package  v1.0.0")
  Test.equal(
    manager.widgetsById.installed_row_1.definition.text,
    "installed-package  v2.0.0 · ✓"
  )

  ui:set_busy(true)
  Test.falsy(manager.widgetsById.browse_stacked_details_1.definition.enabled)
  Test.falsy(manager.widgetsById.installed_stacked_details_1.definition.enabled)
  ui:set_busy(false)
  manager.widgetsById.browse_stacked_details_1.definition.onclick()
  Test.equal(dialogs[2].options.title, "browse-package")

  manager:resize(760)
  timers[#timers]:fire()
  for _, kind in ipairs({ "browse", "installed" }) do
    Test.truthy(manager.widgetsById[kind .. "_details_1"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_stacked_details_1"].definition.visible)
  end

  manager:resize(520)
  local stale_timer = timers[#timers]
  ui:close()
  Test.truthy(stale_timer.stopped)
  stale_timer:fire()
  Test.equal(#dialogs, 2, "responsive layout must not rebuild the manager")
end)

Test.case("compact pagers navigate browse and installed independently", function()
  local Dialog, dialogs = Fakes.dialog_factory()
  local model = Model.new(2)
  model:set_catalog({
    { name = "browse-one", version = "1.0.0" },
    { name = "browse-two", version = "1.0.0" },
    { name = "browse-three", version = "1.0.0" },
  }, "current", false, false)
  model:set_installed({
    { name = "installed-one", version = "1.0.0" },
    { name = "installed-two", version = "1.0.0" },
    { name = "installed-three", version = "1.0.0" },
  })
  local ui = Ui.new({
    app = Fakes.app {
      allFilesExist = true,
    },
    plugin = Fakes.plugin(),
    Dialog = Dialog,
  }, model)
  ui:open {
    refresh = function() end,
    install_from_github = function() end,
    sync_local_folder = function() end,
    on_dialog_closed = function() end,
  }

  local manager = dialogs[1]
  for _, kind in ipairs({ "browse", "installed" }) do
    Test.equal(manager.widgetsById[kind .. "_page"].definition.text, "1 / 2")
    Test.truthy(manager.widgetsById[kind .. "_page"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_previous"].definition.enabled)
    Test.truthy(manager.widgetsById[kind .. "_next"].definition.enabled)
  end

  manager.widgetsById.browse_next.definition.onclick()
  Test.equal(manager.widgetsById.browse_page.definition.text, "2 / 2")
  Test.truthy(manager.widgetsById.browse_previous.definition.enabled)
  Test.falsy(manager.widgetsById.browse_next.definition.enabled)
  Test.contains(manager.widgetsById.browse_row_1.definition.text, "browse-three")
  Test.equal(manager.widgetsById.installed_page.definition.text, "1 / 2")

  manager.widgetsById.browse_previous.definition.onclick()
  Test.equal(manager.widgetsById.browse_page.definition.text, "1 / 2")
  Test.falsy(manager.widgetsById.browse_previous.definition.enabled)
  Test.truthy(manager.widgetsById.browse_next.definition.enabled)

  manager.widgetsById.installed_next.definition.onclick()
  Test.equal(manager.widgetsById.installed_page.definition.text, "2 / 2")
  Test.truthy(manager.widgetsById.installed_previous.definition.enabled)
  Test.falsy(manager.widgetsById.installed_next.definition.enabled)
  Test.contains(manager.widgetsById.installed_row_1.definition.text, "installed-three")
  Test.equal(manager.widgetsById.browse_page.definition.text, "1 / 2")

  ui:set_busy(true)
  Test.falsy(manager.widgetsById.browse_next.definition.enabled)
  Test.falsy(manager.widgetsById.installed_previous.definition.enabled)
end)

Test.case("compact pagers remain one native button row without samerow support", function()
  local BaseDialog, dialogs = Fakes.dialog_factory()
  local function Dialog(options)
    local dialog = BaseDialog(options)
    dialog.samerow = nil
    return dialog
  end
  local model = Model.new(1)
  model:set_catalog({
    { name = "one", version = "1.0.0" },
    { name = "two", version = "1.0.0" },
  }, "current", false, false)
  local ui = Ui.new({
    app = Fakes.app {
      allFilesExist = true,
    },
    plugin = Fakes.plugin(),
    Dialog = Dialog,
  }, model)
  ui:open {
    refresh = function() end,
    install_from_github = function() end,
    sync_local_folder = function() end,
    on_dialog_closed = function() end,
  }

  local manager = dialogs[1]
  Test.equal(manager.widgetsById.browse_previous.kind, "button")
  Test.equal(manager.widgetsById.browse_page.kind, "button")
  Test.equal(manager.widgetsById.browse_next.kind, "button")
  Test.equal(manager.widgetsById.browse_previous.definition.text, "←")
  Test.equal(manager.widgetsById.browse_page.definition.text, "1 / 2")
  Test.equal(manager.widgetsById.browse_next.definition.text, "→")
  Test.truthy(manager.widgetsById.browse_details_1.definition.visible)
  Test.falsy(manager.widgetsById.browse_stacked_details_1)
end)

Test.case("secondary windows are resizable and scroll when space is constrained", function()
  local Dialog, dialogs = Fakes.dialog_factory()
  local ui = Ui.new({
    app = Fakes.app {
      allFilesExist = true,
    },
    plugin = Fakes.plugin(),
    Dialog = Dialog,
  }, Model.new(1))
  ui.controller = {
    clear_cache = function() end,
  }

  ui:show_onboarding()
  ui:show_error("Example Error", {
    code = "example",
    message = "A long error remains reachable.",
  })
  ui:confirm("Confirm", "A long confirmation remains reachable.", "Continue")
  ui:show_progress("Progress", "A long progress message remains reachable.")
  ui:prompt_github_url()
  ui:choose_github_asset({
    {
      id = 1,
      name = "Example extension asset",
    },
  })
  ui:prompt_package_json()
  ui:show_help()
  ui:show_preferences()
  ui:show_package_details({
    name = "example-package",
    version = "1.0.0",
    repository = "soupmasters/example-package",
  }, "browse")

  Test.equal(#dialogs, 10)
  for _, dialog in ipairs(dialogs) do
    Test.truthy(dialog.options.resizeable)
    Test.equal(type(dialog.shown), "table")
    Test.truthy(dialog.shown.autoscrollbars)
  end
end)

Test.case("installed rows use compact management status symbols", function()
  local Dialog, dialogs = Fakes.dialog_factory()
  local model = Model.new(2)
  model:set_installed({
    {
      displayName = "Linked Package",
      version = "1.0.0",
      managed = true,
      enabled = true,
      update = {
        version = "2.0.0",
      },
    },
    {
      displayName = "External Package",
      version = "3.0.0",
      managed = false,
      enabled = false,
      updateError = {
        code = "network",
      },
      rollbackAvailable = true,
    },
  })
  local ui = Ui.new({
    app = Fakes.app {
      allFilesExist = true,
    },
    plugin = Fakes.plugin(),
    Dialog = Dialog,
  }, model)
  ui:open {
    refresh = function() end,
    install_from_github = function() end,
    sync_local_folder = function() end,
    on_dialog_closed = function() end,
  }

  Test.equal(
    dialogs[1].widgetsById.installed_row_1.definition.text,
    "Linked Package  v1.0.0 · ✓ · Enabled · Update 2.0.0"
  )
  Test.equal(
    dialogs[1].widgetsById.installed_row_2.definition.text,
    "External Package  v3.0.0 · ⚠ · Disabled · Update check failed · Restore available"
  )
end)

Test.case("install button opens a native menu and dispatches its choices", function()
  local Dialog, dialogs = Fakes.dialog_factory()
  local ui = Ui.new({
    app = Fakes.app {
      allFilesExist = true,
    },
    plugin = Fakes.plugin(),
    Dialog = Dialog,
  }, Model.new(1))
  local github_count = 0
  local local_count = 0
  ui:open {
    refresh = function() end,
    install_from_github = function()
      github_count = github_count + 1
    end,
    sync_local_folder = function()
      local_count = local_count + 1
    end,
    on_dialog_closed = function() end,
  }

  local manager = dialogs[1]
  manager.widgetsById.install.definition.onclick()
  local github_menu = dialogs[2]
  Test.equal(github_menu.options.parent, manager)
  Test.truthy(github_menu.shownMenu)
  Test.equal(github_menu.widgetsById.install_github.kind, "menuItem")
  Test.equal(github_menu.widgetsById.install_github.definition.text, "GitHub…")
  Test.equal(github_menu.widgetsById.sync_local.kind, "menuItem")
  Test.equal(github_menu.widgetsById.sync_local.definition.text, "Local Folder…")

  github_menu.widgetsById.install_github.definition.onclick()
  Test.equal(github_count, 1)
  Test.equal(local_count, 0)

  manager.widgetsById.install.definition.onclick()
  dialogs[3].widgetsById.sync_local.definition.onclick()
  Test.equal(github_count, 1)
  Test.equal(local_count, 1)

  ui:set_busy(true)
  Test.falsy(manager.widgetsById.install.definition.enabled)
  Test.falsy(manager.widgetsById.refresh.definition.enabled)
  Test.falsy(manager.widgetsById.preferences.definition.enabled)
end)

Test.case("empty bundled catalog points to the install menu", function()
  local app = Fakes.app {
    allFilesExist = true,
  }
  local Dialog, dialogs = Fakes.dialog_factory()
  local model = Model.new(1)
  local ui = Ui.new({
    app = app,
    plugin = Fakes.plugin(),
    Dialog = Dialog,
  }, model)
  ui:open {
    refresh = function() end,
    install_from_github = function() end,
    sync_local_folder = function() end,
    on_dialog_closed = function() end,
  }
  local empty = dialogs[1].widgetsById.browse_empty.definition
  Test.contains(empty.text, "Catalog is empty")
  Test.contains(empty.text, "Use Install…")
end)

Test.case("catalog author objects render by name", function()
  local app = Fakes.app {
    allFilesExist = true,
  }
  local Dialog, dialogs = Fakes.dialog_factory()
  local model = Model.new(1)
  model:set_catalog({
    {
      name = "example",
      displayName = "Example",
      version = "1.0.0",
      author = {
        name = "Package Author",
      },
    },
  }, "current", false, false)
  local ui = Ui.new({
    app = app,
    plugin = Fakes.plugin(),
    Dialog = Dialog,
  }, model)
  ui:open {
    refresh = function() end,
    install_from_github = function() end,
    sync_local_folder = function() end,
    on_dialog_closed = function() end,
  }
  local row = dialogs[1].widgetsById.browse_row_1.definition.text
  Test.contains(row, "Package Author")
  Test.falsy(row:find("table:", 1, true))
end)

Test.case("catalog manifest names are searchable and shown in details", function()
  local app = Fakes.app {
    allFilesExist = true,
  }
  local Dialog, dialogs = Fakes.dialog_factory()
  local model = Model.new(1)
  model:set_catalog({
    {
      id = "example-tools",
      manifestName = "Example-Tools",
      displayName = "Example Tools",
      version = "1.0.0",
    },
  }, "current", false, false)
  model:set_search("browse", "example-tools")
  local matches = model:filtered("browse")
  Test.equal(#matches, 1)

  local ui = Ui.new({
    app = app,
    plugin = Fakes.plugin(),
    Dialog = Dialog,
  }, model)
  ui:show_package_details(matches[1], "browse")
  Test.equal(dialogs[1].widgets[1].definition.text, "Example-Tools")
end)

Test.case("installed details expose explicit native lifecycle handoffs", function()
  local app = Fakes.app {
    allFilesExist = true,
  }
  local Dialog, dialogs = Fakes.dialog_factory()
  local model = Model.new(1)
  local ui = Ui.new({
    app = app,
    plugin = Fakes.plugin(),
    Dialog = Dialog,
  }, model)
  ui:open {
    refresh = function() end,
    install_from_github = function() end,
    sync_local_folder = function() end,
    on_dialog_closed = function() end,
    open_native_extension_preferences = function() end,
  }
  ui:show_package_details({
    name = "example",
    displayName = "Example",
    version = "1.0.0",
    managed = false,
  }, "installed")

  local buttons = {}
  for _, widget in ipairs(dialogs[2].widgets) do
    if widget.kind == "button" then
      buttons[widget.definition.text] = true
    end
  end
  Test.truthy(buttons["Enable / Disable…"])
  Test.truthy(buttons["Uninstall…"])
end)

Test.case("manager details expose self-update and recovery even when unmanaged", function()
  local app = Fakes.app {
    allFilesExist = true,
  }
  local Dialog, dialogs = Fakes.dialog_factory()
  local model = Model.new(1)
  local ui = Ui.new({
    app = app,
    plugin = Fakes.plugin(),
    Dialog = Dialog,
  }, model)
  local updated = 0
  local restored = 0
  ui:open {
    refresh = function() end,
    install_from_github = function() end,
    sync_local_folder = function() end,
    on_dialog_closed = function() end,
    update_package = function()
      updated = updated + 1
    end,
    restore_package = function()
      restored = restored + 1
    end,
    open_native_extension_preferences = function() end,
  }
  ui:show_package_details({
    name = "aseprite-extension-manager",
    displayName = "Aseprite Extension Manager",
    version = "0.1.0",
    managed = false,
    rollbackAvailable = true,
    update = {
      kind = "self",
      version = "0.2.0",
    },
    updateError = {
      code = "network",
      message = "Could not check the official release.",
      details = {
        recoveryArtifact = "/state/self-update/recovery.aseprite-extension",
      },
    },
  }, "installed")

  local details = dialogs[2]
  local buttons = {}
  local labels = {}
  for _, widget in ipairs(details.widgets) do
    if widget.kind == "button" then
      buttons[widget.definition.text] = widget.definition
    elseif widget.kind == "label" then
      labels[#labels + 1] = widget.definition
    end
  end
  Test.truthy(buttons["Update Manager…"])
  Test.truthy(buttons["Restore Manager…"])
  local saw_update_error = false
  local saw_recovery = false
  for _, label in ipairs(labels) do
    if label.label == "Update check:" then
      saw_update_error = label.text == "Could not check the official release."
    elseif label.label == "Recovery package:" then
      saw_recovery = label.text == "/state/self-update/recovery.aseprite-extension"
    end
  end
  Test.truthy(saw_update_error)
  Test.truthy(saw_recovery)

  buttons["Update Manager…"].onclick()
  Test.equal(updated, 1)
  buttons["Restore Manager…"].onclick()
  Test.equal(restored, 1)
end)

Test.case("model retains update source errors from update checks", function()
  local model = Model.new(1)
  model:set_update_errors({
    {
      packageName = "example",
      error = {
        code = "network",
      },
    },
  })
  Test.equal(#model.updateErrors, 1)
  Test.equal(model.updateErrors[1].packageName, "example")
end)

Test.case("native manager close delegates lifecycle cleanup to controller", function()
  local app = Fakes.app {
    allFilesExist = true,
  }
  local Dialog, dialogs = Fakes.dialog_factory()
  local model = Model.new(1)
  local ui = Ui.new({
    app = app,
    plugin = Fakes.plugin(),
    Dialog = Dialog,
  }, model)
  local close_count = 0
  ui:open {
    refresh = function() end,
    install_from_github = function() end,
    sync_local_folder = function() end,
    on_dialog_closed = function()
      close_count = close_count + 1
    end,
  }
  Test.falsy(dialogs[1].widgetsById.close)
  dialogs[1]:close()
  Test.equal(close_count, 1)
  Test.falsy(ui.dialog)
end)

Test.case("GitHub asset chooser returns numeric identifiers as strings", function()
  local app = Fakes.app {
    allFilesExist = true,
  }
  local BaseDialog, dialogs = Fakes.dialog_factory()
  local function Dialog(options)
    local dialog = BaseDialog(options)
    local base_show = dialog.show
    function dialog:show(show_options)
      self.data.github_asset = "Example package"
      self.data.choose_github_asset = true
      return base_show(self, show_options)
    end
    return dialog
  end
  local ui = Ui.new({
    app = app,
    plugin = Fakes.plugin(),
    Dialog = Dialog,
  }, Model.new(1))

  local selected = ui:choose_github_asset({
    {
      id = 42,
      name = "Example package",
    },
  })
  Test.equal(selected, "42")
  Test.equal(#dialogs, 1)
end)

Test.case("restricted generic manager installs can open the trusted release page", function()
  local app, calls = Fakes.app {
    allFilesExist = true,
  }
  local Dialog, dialogs = Fakes.dialog_factory()
  local ui = Ui.new({
    app = app,
    plugin = Fakes.plugin(),
    Dialog = Dialog,
  }, Model.new(1))

  ui:show_error("Manager Update", {
    code = "SELF_UPDATE_RESTRICTED",
    message = "Use the dedicated manager update action.",
    details = {
      releaseUrl = "https://github.com/soupmasters/AsepriteExtensionManager/releases",
    },
  })
  dialogs[1].widgets[3].definition.onclick()

  Test.equal(#calls.launch, 1)
  Test.equal(
    calls.launch[1].path,
    "https://github.com/soupmasters/AsepriteExtensionManager/releases"
  )
end)
