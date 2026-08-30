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

Test.case("browse sorting and compatibility filters compose before paging", function()
  local model = Model.new(1)
  model:set_catalog({
    {
      id = "charlie",
      displayName = "Charlie",
      latest = nil,
    },
    {
      id = "alpha",
      displayName = "alpha",
      latest = {
        publishedAt = "2025-01-01T00:00:00Z",
      },
    },
    {
      id = "beta",
      displayName = "Beta",
      latest = {
        publishedAt = "2026-01-01T00:00:00Z",
      },
    },
  }, "current", false, false)

  local alphabetical = model:filtered("browse")
  Test.equal(alphabetical[1].id, "alpha")
  Test.equal(alphabetical[2].id, "beta")
  Test.equal(alphabetical[3].id, "charlie")
  Test.equal(model.catalog[1].id, "charlie", "sorting must not mutate catalog order")

  model:move_page("browse", 2)
  Test.equal(model.browsePage, 3)
  Test.truthy(model:set_sort("browse", "name_desc"))
  Test.equal(model.browsePage, 1)
  local descending = model:filtered("browse")
  Test.equal(descending[1].id, "charlie")
  Test.equal(descending[3].id, "alpha")

  Test.truthy(model:set_sort("browse", "recent"))
  local recent = model:filtered("browse")
  Test.equal(recent[1].id, "beta")
  Test.equal(recent[2].id, "alpha")
  Test.equal(recent[3].id, "charlie")

  model:move_page("browse", 2)
  Test.truthy(model:set_filter("browse", "compatible"))
  Test.equal(model.browsePage, 1)
  local compatible, page, pages, total = model:page("browse")
  Test.equal(compatible[1].id, "beta")
  Test.equal(page, 1)
  Test.equal(pages, 2)
  Test.equal(total, 2)
  model:move_page("browse", 1)
  Test.falsy(model:set_filter("browse", "compatible"))
  Test.equal(model.browsePage, 2)

  model:set_search("browse", "alpha")
  local searched = model:filtered("browse")
  Test.equal(#searched, 1)
  Test.equal(searched[1].id, "alpha")

  model:set_search("browse", "")
  Test.truthy(model:set_filter("browse", "unavailable"))
  local unavailable = model:filtered("browse")
  Test.equal(#unavailable, 1)
  Test.equal(unavailable[1].id, "charlie")

  Test.falsy(model:set_filter("browse", "unknown"))
  Test.equal(model.browseFilter, "unavailable")
  Test.falsy(model:set_filter("browse", "unavailable"))
  Test.falsy(model:set_sort("unknown", "name_asc"))
end)

Test.case("installed sorting and status filters compose before paging", function()
  local model = Model.new(2)
  model:set_installed({
    {
      name = "zulu",
      displayName = "Zulu",
      updateError = {
        code = "offline",
      },
    },
    {
      name = "alpha",
      displayName = "Alpha",
      managed = true,
    },
    {
      name = "beta",
      displayName = "Beta",
      managed = false,
      update = {
        version = "2.0.0",
      },
    },
    {
      name = "delta",
      displayName = "Delta",
      managed = true,
      update = "available",
    },
  })

  local alphabetical = model:filtered("installed")
  Test.equal(alphabetical[1].name, "alpha")
  Test.equal(alphabetical[4].name, "zulu")
  Test.equal(model.installed[1].name, "zulu", "sorting must not mutate installed order")

  model:move_page("installed", 1)
  Test.equal(model.installedPage, 2)
  Test.truthy(model:set_sort("installed", "updates_first"))
  Test.equal(model.installedPage, 1)
  local updates_first = model:filtered("installed")
  Test.equal(updates_first[1].name, "beta")
  Test.equal(updates_first[2].name, "delta")
  Test.equal(updates_first[3].name, "alpha")

  Test.truthy(model:set_filter("installed", "updates"))
  local updates = model:filtered("installed")
  Test.equal(#updates, 2)
  Test.equal(updates[1].name, "beta")
  Test.equal(updates[2].name, "delta")

  Test.truthy(model:set_filter("installed", "managed"))
  local managed = model:filtered("installed")
  Test.equal(#managed, 2)
  Test.equal(managed[1].name, "delta")
  Test.equal(managed[2].name, "alpha")

  Test.truthy(model:set_filter("installed", "unmanaged"))
  model:set_search("installed", "zulu")
  local unmanaged = model:filtered("installed")
  Test.equal(#unmanaged, 1)
  Test.equal(unmanaged[1].name, "zulu")
  Test.falsy(model:set_sort("installed", "recent"))
  Test.equal(model.installedSort, "updates_first")
  Test.falsy(model:set_sort("installed", "updates_first"))
end)

Test.case("manager is excluded from both package lists regardless of source", function()
  local manager_catalog = {
    id = "ASEPRITE-EXTENSION-MANAGER",
    manifestName = "Aseprite-Extension-Manager",
    displayName = "Aseprite Extension Manager",
  }
  local manager_installed = {
    name = "Aseprite-Extension-Manager",
    isSelf = true,
    displayName = "Aseprite Extension Manager",
    version = "0.2.0",
    source = {
      kind = "local",
      packageJsonPath = "/work/manager/package.json",
    },
    update = {
      kind = "self",
      version = "0.3.0",
    },
  }
  local model = Model.new(1)
  model:set_catalog({
    manager_catalog,
    {
      id = "example",
      manifestName = "example",
      displayName = "Example",
    },
  }, "current", false, false)
  model:set_installed({
    manager_installed,
    {
      name = "example",
      displayName = "Example",
      source = {
        kind = "github-release",
      },
    },
  })
  model:set_update_errors({
    {
      packageName = "ASEPRITE-EXTENSION-MANAGER",
      error = {
        code = "network",
      },
    },
    {
      packageName = "example",
      error = {
        code = "network",
      },
    },
  })

  Test.equal(#model.catalog, 1)
  Test.equal(model.catalog[1].id, "example")
  Test.equal(model.managerCatalogPackage, manager_catalog)
  Test.equal(#model.installed, 1)
  Test.equal(model.installed[1].name, "example")
  Test.equal(model.managerPackage, manager_installed)
  Test.equal(model.managerPackage.update.kind, "self")
  Test.equal(#model.updateErrors, 1)
  Test.equal(model.updateErrors[1].packageName, "example")

  local browse, _, _, browse_total = model:page("browse")
  local installed, _, _, installed_total = model:page("installed")
  Test.equal(#browse, 1)
  Test.equal(browse_total, 1)
  Test.equal(#installed, 1)
  Test.equal(installed_total, 1)

  model:set_search("browse", "aseprite-extension-manager")
  model:set_search("installed", "aseprite extension manager")
  Test.equal(#model:filtered("browse"), 0)
  Test.equal(#model:filtered("installed"), 0)

  Test.truthy(Model.is_manager_installed_package({
    name = "ASEPRITE-EXTENSION-MANAGER",
    isSelf = true,
  }))
  Test.falsy(Model.is_manager_installed_package({
    name = "aseprite-extension-manager",
    isSelf = false,
  }))
  Test.truthy(Model.is_manager_catalog_package({
    id = "aseprite-extension-manager",
    manifestName = "aseprite-extension-manager",
  }))
  Test.falsy(Model.is_manager_catalog_package({
    id = "aseprite-extension-manager",
  }))
  Test.falsy(Model.is_manager_catalog_package({
    displayName = "Aseprite Extension Manager",
  }))
end)

Test.case("manager uses compact native install and utility controls", function()
  local app, calls = Fakes.app {
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
  Test.equal(manager.widgetsById.manager_update.definition.text, "↑")
  Test.falsy(manager.widgetsById.manager_update.definition.visible)
  Test.falsy(manager.widgetsById.manager_update.definition.hexpand)
  Test.truthy(widget_indices.install < widget_indices.manager_update)
  Test.truthy(widget_indices.manager_update < widget_indices.refresh)
  Test.truthy(buttons["↻"])
  Test.truthy(buttons["⚙"])
  Test.truthy(buttons["⍰"])
  Test.falsy(manager.widgetsById.install_github)
  Test.falsy(manager.widgetsById.sync_local)
  Test.falsy(manager.widgetsById.close)

  local help = manager.widgetsById.help
  Test.equal(help.kind, "button")
  Test.equal(help.definition.text, "⍰")
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
  Test.equal(help_dialog.widgetsById.help_license.definition.text, "MIT License")
  local repository_link = help_dialog.widgetsById.help_repository
  Test.equal(repository_link.kind, "canvas")
  Test.equal(repository_link.definition.label, "Repository:")
  Test.falsy(repository_link.definition.hexpand)
  Test.falsy(repository_link.definition.vexpand)

  local painted = {
    moves = {},
  }
  local context = {
    width = 120,
    height = 16,
    theme = {
      color = {
        link_text = "link-text",
        link_hover = "link-hover",
      },
    },
  }
  function context:measureText(value)
    painted.measured = value
    return {
      width = 96,
      height = 12,
    }
  end
  function context:fillText(value, x, y)
    painted.text = value
    painted.textX = x
    painted.textY = y
  end
  function context:beginPath()
    painted.beganPath = true
  end
  function context:moveTo(x, y)
    painted.moves[#painted.moves + 1] = {
      x,
      y,
    }
  end
  function context:lineTo(x, y)
    painted.moves[#painted.moves + 1] = {
      x,
      y,
    }
  end
  function context:stroke()
    painted.stroked = true
  end
  repository_link.definition.onpaint {
    context = context,
  }
  Test.equal(painted.measured, "View on GitHub")
  Test.equal(painted.text, "View on GitHub")
  Test.equal(context.color, "link-text")
  Test.truthy(painted.beganPath)
  Test.truthy(painted.stroked)

  repository_link.definition.onmousedown {
    button = 1,
    x = 2,
    y = 2,
  }
  repository_link.definition.onmouseup {
    button = 1,
    x = 2,
    y = 2,
  }
  Test.equal(#calls.launch, 1)
  Test.equal(
    calls.launch[1].path,
    "https://github.com/soupmasters/AsepriteExtensionManager"
  )

  repository_link.definition.onmousedown {
    button = 2,
    x = 2,
    y = 2,
  }
  repository_link.definition.onmouseup {
    button = 2,
    x = 2,
    y = 2,
  }
  repository_link.definition.onmousedown {
    button = 1,
    x = 2,
    y = 2,
  }
  repository_link.definition.onmouseup {
    button = 1,
    x = 121,
    y = 2,
  }
  Test.equal(#calls.launch, 1)
  Test.equal(help_dialog.repaintCount, 6)
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

Test.case("sort and filter controls update browse and installed independently", function()
  local Dialog, dialogs = Fakes.dialog_factory()
  local model = Model.new(1)
  model:set_catalog({
    {
      id = "zulu-catalog",
      displayName = "Zulu Catalog",
      latest = {
        version = "2.0.0",
        publishedAt = "2026-01-01T00:00:00Z",
      },
    },
    {
      id = "alpha-catalog",
      displayName = "Alpha Catalog",
    },
  }, "current", false, false)
  model:set_installed({
    {
      name = "zulu-installed",
      displayName = "Zulu Installed",
      version = "1.0.0",
      managed = true,
    },
    {
      name = "alpha-installed",
      displayName = "Alpha Installed",
      version = "1.0.0",
      managed = false,
      update = {
        version = "2.0.0",
      },
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

  local manager = dialogs[1]
  for _, id in ipairs({
    "browse_filter",
    "browse_sort",
    "installed_filter",
    "installed_sort",
  }) do
    Test.equal(manager.widgetsById[id].kind, "combobox")
    Test.truthy(manager.widgetsById[id].definition.visible)
  end
  Test.equal(manager.widgetsById.browse_filter.definition.option, "All")
  Test.equal(manager.widgetsById.browse_sort.definition.option, "Name A-Z")
  Test.equal(manager.widgetsById.installed_filter.definition.option, "All")
  Test.equal(manager.widgetsById.installed_sort.definition.option, "Name A-Z")
  Test.contains(manager.widgetsById.browse_row_1.definition.text, "Alpha Catalog")
  Test.contains(manager.widgetsById.installed_row_1.definition.text, "Alpha Installed")

  model:move_page("browse", 1)
  ui:refresh()
  Test.equal(manager.widgetsById.browse_page.definition.text, "2 / 2")
  manager.data.browse_filter = "Compatible"
  manager.widgetsById.browse_filter.definition.onchange()
  Test.equal(model.browseFilter, "compatible")
  Test.equal(model.browsePage, 1)
  Test.contains(manager.widgetsById.browse_row_1.definition.text, "Zulu Catalog")
  Test.equal(manager.widgetsById.browse_stacked_filter.definition.option, "Compatible")

  manager.data.browse_sort = "Newest"
  manager.widgetsById.browse_sort.definition.onchange()
  Test.equal(model.browseSort, "recent")
  Test.equal(model.installedSort, "name_asc")

  manager.data.installed_filter = "Updates Available"
  manager.widgetsById.installed_filter.definition.onchange()
  Test.equal(model.installedFilter, "updates")
  Test.contains(manager.widgetsById.installed_row_1.definition.text, "Alpha Installed")
  Test.equal(model.browseFilter, "compatible")

  manager.data.installed_filter = "All"
  manager.widgetsById.installed_filter.definition.onchange()
  manager.data.installed_sort = "Name Z-A"
  manager.widgetsById.installed_sort.definition.onchange()
  Test.equal(model.installedSort, "name_desc")
  Test.contains(manager.widgetsById.installed_row_1.definition.text, "Zulu Installed")

  model:set_catalog({
    {
      id = "compatible-only",
      displayName = "Compatible Only",
      latest = {
        version = "1.0.0",
        publishedAt = "2026-01-01T00:00:00Z",
      },
    },
  }, "current", false, false)
  model:set_filter("browse", "unavailable")
  ui:refresh()
  Test.truthy(manager.widgetsById.browse_empty.definition.visible)
  Test.equal(
    manager.widgetsById.browse_empty.definition.text,
    "No catalog packages match this filter."
  )

  model:set_search("browse", "missing")
  ui:refresh()
  Test.equal(
    manager.widgetsById.browse_empty.definition.text,
    "No catalog packages match this search and filter."
  )

  model.registryExpired = true
  ui:refresh()
  Test.equal(
    manager.widgetsById.browse_empty.definition.text,
    "No catalog packages match this search and filter. "
      .. "Catalog metadata is expired. Cached entries are view-only."
  )
end)

Test.case("persistent manager update arrow dispatches the hidden self update", function()
  local Dialog, dialogs = Fakes.dialog_factory()
  local model = Model.new(1)
  local manager_package = {
    name = "aseprite-extension-manager",
    displayName = "Aseprite Extension Manager",
    version = "0.1.0",
    isSelf = true,
    managed = false,
    update = {
      kind = "self",
      version = "0.2.0",
    },
  }
  model:set_installed({
    manager_package,
  })
  local ui = Ui.new({
    app = Fakes.app {
      allFilesExist = true,
    },
    plugin = Fakes.plugin(),
    Dialog = Dialog,
  }, model)
  local dispatched
  ui:open {
    refresh = function() end,
    install_from_github = function() end,
    sync_local_folder = function() end,
    update_package = function(_, package)
      dispatched = package
    end,
    on_dialog_closed = function() end,
  }

  local manager = dialogs[1]
  local update_button = manager.widgetsById.manager_update
  Test.equal(update_button.definition.text, "↑")
  Test.truthy(update_button.definition.visible)
  Test.truthy(update_button.definition.enabled)
  Test.contains(manager.widgetsById.manager_status.definition.text, "v0.2.0 available")
  update_button.definition.onclick()
  Test.equal(dispatched, manager_package)

  ui:set_busy(true)
  Test.truthy(update_button.definition.visible)
  Test.falsy(update_button.definition.enabled)

  model.busy = false
  manager_package.update = nil
  ui:refresh()
  Test.falsy(update_button.definition.visible)
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
    Test.truthy(manager.widgetsById[kind .. "_filter"].definition.visible)
    Test.truthy(manager.widgetsById[kind .. "_sort"].definition.visible)
    Test.truthy(manager.widgetsById[kind .. "_filter_label"].definition.visible)
    Test.truthy(manager.widgetsById[kind .. "_sort_label"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_stacked_filter"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_stacked_sort"].definition.visible)
  end

  manager:resize(520)
  local narrow_timer = timers[#timers]
  Test.truthy(narrow_timer.started)
  narrow_timer:fire()
  for _, kind in ipairs({ "browse", "installed" }) do
    Test.falsy(manager.widgetsById[kind .. "_details_1"].definition.visible)
    Test.truthy(manager.widgetsById[kind .. "_stacked_details_1"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_filter"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_sort"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_filter_label"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_sort_label"].definition.visible)
    Test.truthy(manager.widgetsById[kind .. "_stacked_filter"].definition.visible)
    Test.truthy(manager.widgetsById[kind .. "_stacked_sort"].definition.visible)
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
    Test.truthy(manager.widgetsById[kind .. "_filter"].definition.visible)
    Test.truthy(manager.widgetsById[kind .. "_sort"].definition.visible)
    Test.truthy(manager.widgetsById[kind .. "_filter_label"].definition.visible)
    Test.truthy(manager.widgetsById[kind .. "_sort_label"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_stacked_filter"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_stacked_sort"].definition.visible)
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
  Test.contains(manager.widgetsById.browse_row_1.definition.text, "browse-two")
  Test.equal(manager.widgetsById.installed_page.definition.text, "1 / 2")

  manager.widgetsById.browse_previous.definition.onclick()
  Test.equal(manager.widgetsById.browse_page.definition.text, "1 / 2")
  Test.falsy(manager.widgetsById.browse_previous.definition.enabled)
  Test.truthy(manager.widgetsById.browse_next.definition.enabled)

  manager.widgetsById.installed_next.definition.onclick()
  Test.equal(manager.widgetsById.installed_page.definition.text, "2 / 2")
  Test.truthy(manager.widgetsById.installed_previous.definition.enabled)
  Test.falsy(manager.widgetsById.installed_next.definition.enabled)
  Test.contains(manager.widgetsById.installed_row_1.definition.text, "installed-two")
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
  Test.truthy(manager.widgetsById.browse_filter.definition.visible)
  Test.truthy(manager.widgetsById.browse_sort.definition.visible)
  Test.falsy(manager.widgetsById.browse_stacked_filter)
  Test.falsy(manager.widgetsById.browse_stacked_sort)
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
    "External Package  v3.0.0 · ⚠ · Disabled · Update check failed · Restore available"
  )
  Test.equal(
    dialogs[1].widgetsById.installed_row_2.definition.text,
    "Linked Package  v1.0.0 · ✓ · Enabled · Update 2.0.0"
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

Test.case("help keeps hidden manager update and recovery actions available", function()
  local Dialog, dialogs = Fakes.dialog_factory()
  local model = Model.new(1)
  local manager = {
    name = "aseprite-extension-manager",
    isSelf = true,
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
  }
  model:set_installed({
    manager,
  })
  Test.equal(#model.installed, 0)

  local ui = Ui.new({
    app = Fakes.app {
      allFilesExist = true,
    },
    plugin = Fakes.plugin(),
    Dialog = Dialog,
  }, model)
  local updated = 0
  local restored = 0
  ui.controller = {
    update_package = function(_, package)
      Test.equal(package, manager)
      updated = updated + 1
    end,
    restore_package = function(_, package)
      Test.equal(package, manager)
      restored = restored + 1
    end,
  }
  ui:show_help()

  local help = dialogs[1]
  local buttons = {}
  local labels = {}
  for _, widget in ipairs(help.widgets) do
    if widget.kind == "button" then
      buttons[widget.definition.text] = widget.definition
    elseif widget.kind == "label" then
      labels[widget.definition.label or ""] = widget.definition.text
    end
  end
  Test.truthy(buttons["Update Manager…"])
  Test.truthy(buttons["Restore Manager…"])
  Test.falsy(buttons["Enable / Disable…"])
  Test.falsy(buttons["Uninstall…"])
  Test.equal(labels["Update:"], "0.2.0")
  Test.equal(labels["Update check:"], "Could not check the official release.")
  Test.equal(
    labels["Recovery package:"],
    "/state/self-update/recovery.aseprite-extension"
  )

  buttons["Update Manager…"].onclick()
  buttons["Restore Manager…"].onclick()
  Test.equal(updated, 1)
  Test.equal(restored, 1)
end)

Test.case("manager details never expose native disable or uninstall handoffs", function()
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
    isSelf = true,
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
  Test.falsy(buttons["Enable / Disable…"])
  Test.falsy(buttons["Uninstall…"])
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
