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

Test.case("GitHub repository pages stay separate from installed view state", function()
  local model = Model.new(2)
  model.installedSearch = "installed query"
  model.installedPage = 3
  model:set_search("github", "animation")
  model:set_github_page({
    {
      nameWithOwner = "example/private-extension",
      url = "https://github.com/example/private-extension",
      isPrivate = true,
    },
    {
      nameWithOwner = "example/public-extension",
      url = "https://github.com/example/public-extension",
      isPrivate = false,
    },
  }, 2, 5, true, "next==")

  local repositories, page, pages, total = model:page("github")
  Test.equal(#repositories, 2)
  Test.equal(page, 2)
  Test.equal(pages, 3)
  Test.equal(total, 5)
  Test.equal(model.githubSearch, "animation")
  Test.equal(model.githubEndCursor, "next==")
  Test.equal(model.installedSearch, "installed query")
  Test.equal(model.installedPage, 3)
  Test.falsy(model:move_page("github", 1))

  local error_value = {
    code = "GITHUB_CLI_AUTH_REQUIRED",
    message = "Sign in first",
  }
  model:set_github_error(error_value)
  Test.equal(model.githubError, error_value)
  Test.equal(model.githubTotal, 0)
  Test.truthy(model.githubLoaded)
  Test.falsy(model.githubLoading)
  Test.falsy(model:set_search("unknown", "value"))
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
    request_diagnostics = function(_, callback)
      callback({
        tools = {
          git = {
            installed = true,
            version = "2.45.0",
          },
          gh = {
            installed = true,
            version = "2.50.0",
            authenticated = true,
          },
        },
      })
      return {
        cancel = function() end,
      }
    end,
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
  Test.truthy(manager.bounds.width >= 720)

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
  Test.equal(
    help_dialog.widgetsById.help_git.definition.text,
    "✓ Installed · 2.45.0"
  )
  Test.equal(
    help_dialog.widgetsById.help_gh.definition.text,
    "✓ Installed · 2.50.0 · ✓ Signed in"
  )
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

  local dialog_count = #dialogs
  manager.widgetsById.preferences.definition.onclick()
  Test.equal(#dialogs, dialog_count)
  Test.equal(ui.screen, "preferences")
  Test.falsy(manager.widgetsById.manager_tabs.definition.visible)
  Test.falsy(manager.widgetsById.manager_footer_separator.definition.visible)
  Test.falsy(manager.widgetsById.manager_status.definition.visible)
  Test.falsy(manager.widgetsById.install.definition.visible)
  Test.falsy(manager.widgetsById.refresh.definition.visible)
  Test.falsy(manager.widgetsById.preferences.definition.visible)
  Test.falsy(manager.widgetsById.help.definition.visible)
  Test.equal(manager.widgetsById.preferences_back.definition.text, "← Back")
  Test.truthy(manager.widgetsById.preferences_back.definition.visible)
  Test.truthy(manager.widgetsById.preferences_title.definition.visible)
  Test.truthy(manager.widgetsById.startup_checks.definition.visible)
  Test.truthy(manager.widgetsById.preferences_description.definition.visible)
  Test.truthy(manager.widgetsById.preferences_local_data.definition.visible)
  Test.truthy(manager.widgetsById.preferences_data_path.definition.visible)
  Test.truthy(manager.widgetsById.preferences_clear_cache.definition.visible)
  Test.contains(
    manager.widgetsById.preferences_data_path.definition.text,
    "extension-manager"
  )
  Test.falsy(manager.widgetsById.save_preferences)
  Test.falsy(manager.widgetsById.cancel_preferences)
  Test.truthy(same_row_after.preferences_header_lead)
  Test.truthy(same_row_after.preferences_back)

  manager.data.startup_checks = false
  manager.widgetsById.startup_checks.definition.onclick()
  Test.falsy(plugin.preferences.startupChecks)
  manager.data.startup_checks = true
  manager.widgetsById.startup_checks.definition.onclick()
  Test.truthy(plugin.preferences.startupChecks)

  manager.widgetsById.preferences_back.definition.onclick()
  Test.equal(ui.screen, "manager")
  Test.equal(ui.activeView, "browse")
  Test.truthy(manager.widgetsById.manager_tabs.definition.visible)
  Test.truthy(manager.widgetsById.manager_footer_separator.definition.visible)
  Test.truthy(manager.widgetsById.manager_status.definition.visible)
  Test.truthy(manager.widgetsById.install.definition.visible)
  Test.falsy(manager.widgetsById.preferences_back.definition.visible)
  Test.falsy(manager.widgetsById.preferences_title.definition.visible)

  for _, kind in ipairs({ "browse", "installed" }) do
    Test.equal(manager.widgetsById[kind .. "_search_label"].kind, "label")
    Test.equal(manager.widgetsById[kind .. "_search_label"].definition.text, "Search:")
    Test.falsy(manager.widgetsById[kind .. "_search"].definition.label)
    Test.truthy(same_row_after[kind .. "_search_label"])
    for index = 1, model.pageSize do
      local lead_id = kind .. "_row_lead_" .. tostring(index)
      local row_id = kind .. "_row_" .. tostring(index)
      local details_id = kind .. "_details_" .. tostring(index)
      local lead = manager.widgetsById[lead_id]
      local row = manager.widgetsById[row_id]
      local details = manager.widgetsById[details_id]
      Test.equal(lead.kind, "label")
      Test.equal(lead.definition.text, "")
      Test.falsy(lead.definition.visible)
      Test.falsy(lead.definition.hexpand)
      Test.equal(row.kind, "label")
      Test.falsy(row.definition.hexpand)
      Test.equal(details.kind, "button")
      Test.equal(details.definition.text, "Details ▾")
      Test.falsy(details.definition.hexpand)
      Test.truthy(same_row_after[lead_id])
      Test.truthy(same_row_after[details_id])
      Test.equal(widget_indices[details_id], widget_indices[lead_id] + 1)
      Test.equal(widget_indices[row_id], widget_indices[details_id] + 1)
      Test.equal(manager.widgets[widget_indices[row_id] + 1].kind, "newrow")
      local stacked_details = manager.widgetsById[kind .. "_stacked_details_" .. tostring(index)]
      Test.equal(stacked_details.kind, "button")
      Test.equal(stacked_details.definition.text, "Details ▾")
      Test.falsy(stacked_details.definition.visible)
      local stacked_index = widget_indices[kind .. "_stacked_details_" .. tostring(index)]
      Test.equal(manager.widgets[stacked_index - 1].kind, "newrow")
    end
  end

  Test.truthy(manager.widgetsById.github_tab.definition.visible)
  Test.truthy(manager.widgetsById.github_tab.definition.enabled)
  Test.equal(manager.widgetsById.github_tab.definition.text, "GitHub")
  for index = 1, model.pageSize do
    Test.equal(
      manager.widgetsById["github_details_" .. tostring(index)].definition.text,
      "Install"
    )
    Test.equal(
      manager.widgetsById["github_stacked_details_" .. tostring(index)].definition.text,
      "Install"
    )
  end

  for _, kind in ipairs({ "browse", "installed", "github" }) do
    manager:selectTab("manager_tabs", kind .. "_tab")
    local previous = manager.widgetsById[kind .. "_previous"].definition
    local page = manager.widgetsById[kind .. "_page"].definition
    local next_page = manager.widgetsById[kind .. "_next"].definition
    Test.equal(previous.text, "←")
    Test.equal(page.text, "1 / 1")
    Test.equal(next_page.text, "→")
    Test.truthy(previous.visible)
    Test.truthy(page.visible)
    Test.truthy(next_page.visible)
    Test.falsy(previous.enabled)
    Test.falsy(next_page.enabled)
    Test.truthy(widget_indices[kind .. "_previous"] < widget_indices[kind .. "_page"])
    Test.truthy(widget_indices[kind .. "_page"] < widget_indices[kind .. "_next"])
    local pager_end = widget_indices[kind .. "_pager_right_spacer"]
      or widget_indices[kind .. "_next"]
    Test.equal(manager.widgets[pager_end + 1].kind, "newrow")
    local next_widget = manager.widgets[pager_end + 2]
    if kind == "browse" then
      Test.equal(next_widget.kind, "tab")
      Test.equal(next_widget.definition.id, "installed_tab")
    elseif kind == "installed" then
      Test.equal(next_widget.kind, "tab")
      Test.equal(next_widget.definition.id, "github_tab")
    else
      Test.equal(next_widget.kind, "endtabs")
      Test.equal(next_widget.definition.id, "manager_tabs")
    end
  end
end)

Test.case("help reports missing tools and ignores diagnostics after close", function()
  local app = Fakes.app {
    allFilesExist = true,
  }
  local Dialog, dialogs = Fakes.dialog_factory()
  local ui = Ui.new({
    app = app,
    plugin = Fakes.plugin(),
    Dialog = Dialog,
  }, Model.new(1))
  local diagnostics_callback
  local diagnostics_cancelled = false
  ui:open {
    refresh = function() end,
    install_from_github = function() end,
    sync_local_folder = function() end,
    request_diagnostics = function(_, callback)
      diagnostics_callback = callback
      return {
        cancel = function()
          diagnostics_cancelled = true
        end,
      }
    end,
    on_dialog_closed = function() end,
  }

  dialogs[1].widgetsById.help.definition.onclick()
  local help_dialog = dialogs[2]
  Test.equal(help_dialog.widgetsById.help_git.definition.text, "? Checking…")
  Test.equal(help_dialog.widgetsById.help_gh.definition.text, "? Checking…")

  diagnostics_callback({
    tools = {
      git = {
        installed = false,
      },
      gh = {
        installed = true,
        version = "2.50.0",
        authenticated = false,
      },
    },
  })
  Test.equal(help_dialog.widgetsById.help_git.definition.text, "✕ Not installed")
  Test.equal(
    help_dialog.widgetsById.help_gh.definition.text,
    "✓ Installed · 2.50.0 · ✕ Sign-in unavailable"
  )

  help_dialog:close()
  Test.truthy(diagnostics_cancelled)
  diagnostics_callback({
    tools = {
      git = {
        installed = true,
      },
      gh = {
        installed = true,
        authenticated = true,
      },
    },
  })
  Test.equal(help_dialog.widgetsById.help_git.definition.text, "✕ Not installed")
  Test.equal(
    help_dialog.widgetsById.help_gh.definition.text,
    "✓ Installed · 2.50.0 · ✕ Sign-in unavailable"
  )
end)

Test.case("GitHub tab requires both tools and browses signed-in repositories", function()
  local Dialog, dialogs = Fakes.dialog_factory()
  local model = Model.new(2)
  local ui = Ui.new({
    app = Fakes.app {
      allFilesExist = true,
    },
    plugin = Fakes.plugin(),
    Dialog = Dialog,
  }, model)
  local diagnostics_callback
  local searches = {}
  local page_moves = {}
  local installed_repository
  local controller = {
    refresh = function() end,
    install_from_github = function() end,
    sync_local_folder = function() end,
    request_diagnostics = function(_, callback)
      diagnostics_callback = callback
      return {
        cancel = function() end,
      }
    end,
    search_github_repositories = function(_, query)
      searches[#searches + 1] = query
      model:set_github_page({
        {
          nameWithOwner = "example/private-extension",
          url = "https://github.com/example/private-extension",
          description = "Private extension",
          isPrivate = true,
          updatedAt = "2026-08-30T12:00:00Z",
        },
      }, 1, 3, true, "next==")
      return true
    end,
    move_github_page = function(_, delta)
      page_moves[#page_moves + 1] = delta
      return true
    end,
    install_github_repository = function(_, repository)
      installed_repository = repository
    end,
    on_dialog_closed = function() end,
  }
  ui:open(controller)

  local manager = dialogs[1]
  Test.falsy(manager.widgetsById.github_tab.definition.visible)
  Test.falsy(manager.widgetsById.github_tab.definition.enabled)
  diagnostics_callback({
    tools = {
      git = { installed = true },
      gh = { installed = true, authenticated = true },
    },
  })
  Test.truthy(manager.widgetsById.github_tab.definition.visible)
  Test.truthy(manager.widgetsById.github_tab.definition.enabled)

  manager:selectTab("manager_tabs", "github_tab")
  Test.equal(ui.activeView, "github")
  Test.equal(#searches, 1)
  Test.equal(searches[1], "")
  Test.truthy(manager.widgetsById.github_search.definition.visible)
  Test.contains(
    manager.widgetsById.github_row_1.definition.text,
    "example/private-extension · Private"
  )
  Test.equal(manager.widgetsById.github_page.definition.text, "1 / 2")
  Test.truthy(manager.widgetsById.github_next.definition.enabled)

  manager:resize(500)
  Test.falsy(manager.widgetsById.github_details_1.definition.visible)
  Test.truthy(manager.widgetsById.github_stacked_details_1.definition.visible)

  manager.data.github_search = "animation"
  manager.widgetsById.github_search.definition.onchange()
  Test.equal(searches[2], "animation")
  manager.widgetsById.github_next.definition.onclick()
  Test.equal(page_moves[1], 1)
  manager.widgetsById.github_details_1.definition.onclick()
  Test.equal(installed_repository.url, "https://github.com/example/private-extension")
end)

Test.case("GitHub tab stays hidden when either required tool is missing", function()
  for _, missing in ipairs({ "git", "gh" }) do
    local Dialog, dialogs = Fakes.dialog_factory()
    local ui = Ui.new({
      app = Fakes.app {
        allFilesExist = true,
      },
      plugin = Fakes.plugin(),
      Dialog = Dialog,
    }, Model.new(1))
    local diagnostics_callback
    ui:open {
      refresh = function() end,
      install_from_github = function() end,
      sync_local_folder = function() end,
      request_diagnostics = function(_, callback)
        diagnostics_callback = callback
        return {
          cancel = function() end,
        }
      end,
      on_dialog_closed = function() end,
    }
    diagnostics_callback({
      tools = {
        git = { installed = missing ~= "git" },
        gh = { installed = missing ~= "gh", authenticated = true },
      },
    })
    Test.falsy(dialogs[1].widgetsById.github_tab.definition.visible)
    Test.falsy(dialogs[1].widgetsById.github_tab.definition.enabled)
  end
end)

Test.case("signed-out GitHub CLI shows the login instruction", function()
  local Dialog, dialogs = Fakes.dialog_factory()
  local ui = Ui.new({
    app = Fakes.app {
      allFilesExist = true,
    },
    plugin = Fakes.plugin(),
    Dialog = Dialog,
  }, Model.new(1))
  local diagnostics_callback
  local search_count = 0
  ui:open {
    refresh = function() end,
    install_from_github = function() end,
    sync_local_folder = function() end,
    request_diagnostics = function(_, callback)
      diagnostics_callback = callback
      return {
        cancel = function() end,
      }
    end,
    search_github_repositories = function()
      search_count = search_count + 1
    end,
    on_dialog_closed = function() end,
  }
  diagnostics_callback({
    tools = {
      git = { installed = true },
      gh = { installed = true, authenticated = false },
    },
  })
  local manager = dialogs[1]
  Test.truthy(manager.widgetsById.github_tab.definition.visible)
  manager:selectTab("manager_tabs", "github_tab")
  Test.equal(search_count, 0)
  Test.falsy(manager.widgetsById.github_search.definition.enabled)
  Test.truthy(manager.widgetsById.github_empty.definition.visible)
  Test.contains(manager.widgetsById.github_empty.definition.text, "gh auth login")
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
    Test.equal(
      manager.widgetsById[id].definition.visible,
      id:find("browse_", 1, true) == 1
    )
    Test.falsy(manager.widgetsById[id].definition.label)
    Test.falsy(manager.widgetsById[id].definition.hexpand)
  end
  Test.falsy(manager.widgetsById.browse_filter_label)
  Test.falsy(manager.widgetsById.browse_sort_label)
  Test.falsy(manager.widgetsById.installed_filter_label)
  Test.falsy(manager.widgetsById.installed_sort_label)
  local same_row_after = {}
  for _, call in ipairs(manager.sameRowCalls) do
    local definition = call.after and call.after.definition
    if definition and definition.id then
      same_row_after[definition.id] = true
    end
  end
  for _, kind in ipairs({ "browse", "installed" }) do
    Test.truthy(same_row_after[kind .. "_search"])
    Test.truthy(same_row_after[kind .. "_filter"])
    Test.truthy(manager.widgetsById[kind .. "_stacked_filter"].definition.hexpand)
    Test.truthy(manager.widgetsById[kind .. "_stacked_sort"].definition.hexpand)
    Test.falsy(manager.widgetsById[kind .. "_stacked_filter"].definition.label)
    Test.falsy(manager.widgetsById[kind .. "_stacked_sort"].definition.label)
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

  manager:selectTab("manager_tabs", "installed_tab")
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
  manager:selectTab("manager_tabs", "browse_tab")
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
    uiScale = 2,
  }
  local Dialog, dialogs = Fakes.dialog_factory {
    sizeHintWidth = 1800,
    shrinkSizeHintOnShow = 120,
  }
  local Timer, timers = Fakes.timer_factory()
  local model = Model.new(1)
  model:set_catalog({
    {
      name = "browse-package",
      version = "1.0.0",
      author = {
        name = "Package Author",
      },
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
  Test.equal(ui.wideRowMinWidth, 1120)
  timers[#timers]:fire()
  local function assert_layout(kind, stacked)
    manager:selectTab("manager_tabs", kind .. "_tab")
    if stacked then
      Test.falsy(manager.widgetsById[kind .. "_details_1"].definition.visible)
      Test.truthy(manager.widgetsById[kind .. "_stacked_details_1"].definition.visible)
      Test.falsy(manager.widgetsById[kind .. "_filter"].definition.visible)
      Test.falsy(manager.widgetsById[kind .. "_sort"].definition.visible)
      Test.truthy(manager.widgetsById[kind .. "_stacked_filter"].definition.visible)
      Test.truthy(manager.widgetsById[kind .. "_stacked_sort"].definition.visible)
      return
    end
    Test.truthy(manager.widgetsById[kind .. "_details_1"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_stacked_details_1"].definition.visible)
    Test.truthy(manager.widgetsById[kind .. "_filter"].definition.visible)
    Test.truthy(manager.widgetsById[kind .. "_sort"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_stacked_filter"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_stacked_sort"].definition.visible)
  end
  for _, kind in ipairs({ "browse", "installed" }) do
    assert_layout(kind, false)
  end

  manager:resize(520)
  local narrow_timer = timers[#timers]
  Test.truthy(narrow_timer.started)
  narrow_timer:fire()
  for _, kind in ipairs({ "browse", "installed" }) do
    assert_layout(kind, true)
  end
  Test.equal(
    manager.widgetsById.browse_row_1.definition.text,
    "browse-package  v1.0.0 · Package Author"
  )
  Test.equal(
    manager.widgetsById.installed_row_1.definition.text,
    "installed-package  v2.0.0 · ✓"
  )

  ui:set_busy(true)
  Test.falsy(manager.widgetsById.browse_stacked_details_1.definition.enabled)
  Test.falsy(manager.widgetsById.installed_stacked_details_1.definition.enabled)
  ui:set_busy(false)
  manager.widgetsById.browse_stacked_details_1.definition.onclick()
  Test.truthy(dialogs[2].shownMenu)
  Test.equal(dialogs[2].options.parent, manager)
  Test.contains(dialogs[2].widgetsById.package_summary.definition.text, "browse-package")

  manager:resize(1520)
  timers[#timers]:fire()
  for _, kind in ipairs({ "browse", "installed" }) do
    assert_layout(kind, false)
  end

  manager:resize(520)
  local stale_timer = timers[#timers]
  manager.bounds.width = 1520
  manager:selectTab("manager_tabs", "browse_tab")
  stale_timer:fire()
  Test.truthy(manager.widgetsById.browse_details_1.definition.visible)
  Test.falsy(manager.widgetsById.browse_stacked_details_1.definition.visible)

  manager:resize(520)
  local close_timer = timers[#timers]
  ui:close()
  Test.truthy(close_timer.stopped)
  close_timer:fire()
  Test.equal(#dialogs, 2, "responsive layout must not rebuild the manager")
end)

Test.case("inactive manager tab controls stay hidden until selected", function()
  local Dialog, dialogs = Fakes.dialog_factory()
  local model = Model.new(1)
  model:set_catalog({
    { name = "browse-package", version = "1.0.0" },
  }, "current", false, false)
  model:set_installed({
    { name = "installed-package", version = "2.0.0", managed = true },
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
  local function assert_inactive_hidden(kind)
    Test.falsy(manager.widgetsById[kind .. "_search_label"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_search"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_filter"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_sort"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_stacked_filter"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_stacked_sort"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_empty"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_row_1"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_details_1"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_stacked_details_1"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_previous"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_page"].definition.visible)
    Test.falsy(manager.widgetsById[kind .. "_next"].definition.visible)
  end

  local function assert_search_visible(kind)
    if manager.widgetsById[kind .. "_search_label"] then
      Test.truthy(manager.widgetsById[kind .. "_search_label"].definition.visible)
    end
    Test.truthy(manager.widgetsById[kind .. "_search"].definition.visible)
  end

  Test.equal(manager.data.manager_tabs, "browse_tab")
  Test.equal(ui.activeView, "browse")
  assert_search_visible("browse")
  assert_inactive_hidden("installed")

  manager:selectTab("manager_tabs", "installed_tab")
  Test.equal(manager.data.manager_tabs, "installed_tab")
  Test.equal(ui.activeView, "installed")
  assert_inactive_hidden("browse")
  assert_search_visible("installed")
  Test.truthy(manager.widgetsById.installed_row_1.definition.visible)
  Test.truthy(manager.widgetsById.installed_details_1.definition.visible)
  Test.falsy(manager.widgetsById.installed_stacked_details_1.definition.visible)
  Test.truthy(manager.widgetsById.installed_filter.definition.visible)
  Test.falsy(manager.widgetsById.installed_stacked_filter.definition.visible)
  Test.truthy(manager.widgetsById.installed_page.definition.visible)

  local dialog_count = #dialogs
  manager.widgetsById.preferences.definition.onclick()
  Test.equal(#dialogs, dialog_count)
  Test.equal(ui.screen, "preferences")
  Test.equal(ui.activeView, "installed")
  assert_inactive_hidden("browse")
  assert_inactive_hidden("installed")
  Test.falsy(manager.widgetsById.manager_tabs.definition.visible)
  Test.truthy(manager.widgetsById.preferences_back.definition.visible)
  manager.widgetsById.preferences_back.definition.onclick()
  Test.equal(ui.screen, "manager")
  Test.equal(ui.activeView, "installed")
  Test.equal(manager.data.manager_tabs, "installed_tab")
  assert_inactive_hidden("browse")
  assert_search_visible("installed")
  Test.truthy(manager.widgetsById.installed_row_1.definition.visible)

  manager:resize(500)
  assert_inactive_hidden("browse")
  Test.falsy(manager.widgetsById.installed_details_1.definition.visible)
  Test.truthy(manager.widgetsById.installed_stacked_details_1.definition.visible)
  Test.falsy(manager.widgetsById.installed_filter.definition.visible)
  Test.truthy(manager.widgetsById.installed_stacked_filter.definition.visible)

  manager:selectTab("manager_tabs", "browse_tab")
  Test.equal(ui.activeView, "browse")
  assert_inactive_hidden("installed")
  assert_search_visible("browse")
  Test.falsy(manager.widgetsById.browse_details_1.definition.visible)
  Test.truthy(manager.widgetsById.browse_stacked_details_1.definition.visible)
  Test.falsy(manager.widgetsById.browse_filter.definition.visible)
  Test.truthy(manager.widgetsById.browse_stacked_filter.definition.visible)
end)

Test.case("manager opens wide by default and stacks on a small Aseprite window", function()
  local function open_manager(window_width)
    local Dialog, dialogs = Fakes.dialog_factory {
      boundsWidth = 500,
    }
    local model = Model.new(1)
    model:set_catalog({
      { name = "browse-package", version = "1.0.0" },
    }, "current", false, false)
    local ui = Ui.new({
      app = Fakes.app {
        allFilesExist = true,
        windowWidth = window_width,
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
    return ui, dialogs[1]
  end

  local wide_ui, wide = open_manager(1000)
  Test.equal(wide.bounds.width, 720)
  Test.falsy(wide_ui.rowsStacked)
  Test.truthy(wide.widgetsById.browse_details_1.definition.visible)
  Test.falsy(wide.widgetsById.browse_stacked_details_1.definition.visible)

  local narrow_ui, narrow = open_manager(540)
  Test.equal(narrow.bounds.width, 508)
  Test.truthy(narrow_ui.rowsStacked)
  Test.falsy(narrow.widgetsById.browse_details_1.definition.visible)
  Test.truthy(narrow.widgetsById.browse_stacked_details_1.definition.visible)
  Test.falsy(narrow.widgetsById.browse_filter.definition.visible)
  Test.truthy(narrow.widgetsById.browse_stacked_filter.definition.visible)
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
    Test.equal(
      manager.widgetsById[kind .. "_page"].definition.visible,
      kind == "browse"
    )
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

  manager:selectTab("manager_tabs", "installed_tab")
  Test.falsy(manager.widgetsById.browse_page.definition.visible)
  Test.truthy(manager.widgetsById.installed_page.definition.visible)
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
  model:set_installed({
    { name = "installed-one", version = "1.0.0", managed = false },
    { name = "installed-two", version = "1.0.0", managed = true },
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
  Test.equal(manager.widgetsById.browse_previous.kind, "button")
  Test.equal(manager.widgetsById.browse_page.kind, "button")
  Test.equal(manager.widgetsById.browse_next.kind, "button")
  Test.equal(manager.widgetsById.browse_previous.definition.text, "←")
  Test.equal(manager.widgetsById.browse_page.definition.text, "1 / 2")
  Test.equal(manager.widgetsById.browse_next.definition.text, "→")
  Test.falsy(manager.widgetsById.browse_row_1)
  Test.falsy(manager.widgetsById.installed_row_1)
  Test.truthy(manager.widgetsById.browse_details_1.definition.visible)
  Test.falsy(manager.widgetsById.installed_details_1.definition.visible)
  Test.contains(manager.widgetsById.browse_details_1.definition.label, "one  v1.0.0")
  Test.contains(
    manager.widgetsById.installed_details_1.definition.label,
    "installed-one  v1.0.0"
  )
  Test.falsy(manager.widgetsById.browse_stacked_details_1)
  Test.falsy(manager.widgetsById.installed_stacked_details_1)
  Test.truthy(manager.widgetsById.browse_filter.definition.visible)
  Test.truthy(manager.widgetsById.browse_sort.definition.visible)
  Test.falsy(manager.widgetsById.browse_filter.definition.label)
  Test.falsy(manager.widgetsById.browse_sort.definition.label)
  Test.truthy(manager.widgetsById.browse_filter.definition.hexpand)
  Test.truthy(manager.widgetsById.browse_sort.definition.hexpand)
  Test.falsy(manager.widgetsById.browse_stacked_filter)
  Test.falsy(manager.widgetsById.browse_stacked_sort)

  manager.widgetsById.browse_next.definition.onclick()
  Test.equal(manager.widgetsById.browse_page.definition.text, "2 / 2")
  Test.contains(manager.widgetsById.browse_details_1.definition.label, "two  v1.0.0")

  manager:selectTab("manager_tabs", "installed_tab")
  Test.falsy(manager.widgetsById.browse_details_1.definition.visible)
  Test.truthy(manager.widgetsById.installed_details_1.definition.visible)
  Test.falsy(manager.widgetsById.browse_filter.definition.visible)
  Test.truthy(manager.widgetsById.installed_filter.definition.visible)
  manager.widgetsById.installed_next.definition.onclick()
  Test.equal(manager.widgetsById.installed_page.definition.text, "2 / 2")
  Test.contains(
    manager.widgetsById.installed_details_1.definition.label,
    "installed-two  v1.0.0"
  )
end)

Test.case("confirmation dialogs use readable wrapped rows and bounded width", function()
  local function open_confirmation(window_width)
    local Dialog, dialogs = Fakes.dialog_factory()
    local ui = Ui.new({
      app = Fakes.app {
        allFilesExist = true,
        windowWidth = window_width,
      },
      plugin = Fakes.plugin(),
      Dialog = Dialog,
    }, Model.new(1))
    ui:confirm(
      "Clear Download Cache",
      table.concat({
        "Clear cached downloads?",
        "",
        "Cached package files that are not needed by the current recovery point "
          .. "will be removed. Installed extensions and linked source folders "
          .. "will not be changed.",
        string.rep("x", 80),
      }, "\n"),
      "Clear Cache",
      "Keep Cache"
    )
    return dialogs[1]
  end

  local dialog = open_confirmation(1280)
  local message_rows = {}
  for index = 1, 20 do
    local widget = dialog.widgetsById["confirm_message_" .. tostring(index)]
    if not widget then
      break
    end
    message_rows[#message_rows + 1] = widget.definition.text
  end
  Test.truthy(#message_rows >= 7)
  for _, line in ipairs(message_rows) do
    Test.falsy(line:find("\n", 1, true))
    Test.truthy(#line <= 64)
  end
  Test.equal(dialog.widgetsById.confirm_action.definition.text, "Clear Cache")
  Test.equal(dialog.widgetsById.cancel_action.definition.text, "Keep Cache")
  Test.truthy(dialog.shown.autoscrollbars)
  Test.truthy(dialog.shown.bounds)
  Test.truthy(dialog.shown.bounds.width >= 560)
  Test.truthy(dialog.shown.bounds.width <= 1248)

  local narrow = open_confirmation(500)
  Test.equal(narrow.shown.bounds.width, 468)
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

  Test.equal(#dialogs, 8)
  Test.equal(dialogs[8].widgetsById.help_git.definition.text, "? Could not check")
  Test.equal(dialogs[8].widgetsById.help_gh.definition.text, "? Could not check")
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

Test.case("catalog identities are searchable and manifest names appear in menus", function()
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
      latest = {
        version = "1.0.0",
      },
      repository = "https://github.com/example/UnityImporterPluginForUnity",
    },
  }, "current", false, false)
  model:set_search("browse", "example-tools")
  local matches = model:filtered("browse")
  Test.equal(#matches, 1)
  model:set_search("browse", "unityimporterpluginforunity")
  matches = model:filtered("browse")
  Test.equal(#matches, 1)

  local ui = Ui.new({
    app = app,
    plugin = Fakes.plugin(),
    Dialog = Dialog,
  }, model)
  local install_requests = 0
  ui.controller = {
    install_registry_package = function(_, package)
      Test.equal(package.id, "example-tools")
      install_requests = install_requests + 1
    end,
  }
  ui:show_package_menu(matches[1], "browse")
  local menu = dialogs[1]
  Test.truthy(menu.shownMenu)
  Test.equal(menu.widgetsById.package_identity.definition.text, "Package: Example-Tools")
  Test.contains(
    menu.widgetsById.package_repository.definition.text,
    "UnityImporterPluginForUnity"
  )
  Test.falsy(menu.widgetsById.package_identity.definition.enabled)
  Test.equal(menu.widgetsById.package_install.definition.text, "Install")
  menu.widgetsById.package_install.definition.onclick()
  Test.equal(install_requests, 1)
end)

Test.case("installed package menu routes uninstall through the helper", function()
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
  local native_handoffs = 0
  local uninstall_requests = 0
  ui:open {
    refresh = function() end,
    install_from_github = function() end,
    sync_local_folder = function() end,
    on_dialog_closed = function() end,
    open_native_extension_preferences = function()
      native_handoffs = native_handoffs + 1
    end,
    uninstall_package = function(_, package)
      Test.equal(package.name, "example")
      uninstall_requests = uninstall_requests + 1
      return true
    end,
  }
  model:set_installed({ {
    name = "example",
    displayName = "Example",
    version = "1.0.0",
    path = "/profile/extensions/example",
    managed = false,
  } })
  ui:refresh()
  dialogs[1]:selectTab("manager_tabs", "installed_tab")
  dialogs[1].widgetsById.installed_details_1.definition.onclick()

  local menu = dialogs[2]
  Test.truthy(menu.shownMenu)
  Test.falsy(menu.shown)
  Test.equal(menu.options.parent, dialogs[1])
  Test.equal(menu.widgetsById.package_enable_disable.kind, "menuItem")
  Test.equal(menu.widgetsById.package_enable_disable.definition.text, "Enable / Disable…")
  Test.equal(menu.widgetsById.package_uninstall.kind, "menuItem")
  Test.equal(menu.widgetsById.package_uninstall.definition.text, "Uninstall")
  Test.falsy(menu.widgetsById.package_summary.definition.enabled)
  menu.widgetsById.package_uninstall.definition.onclick()
  Test.equal(uninstall_requests, 1)
  Test.equal(native_handoffs, 0)
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

Test.case("manager menu never exposes native disable or uninstall handoffs", function()
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
  ui:show_package_menu({
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

  local menu = dialogs[2]
  Test.truthy(menu.shownMenu)
  Test.equal(menu.widgetsById.package_update_action.definition.text, "Update Manager…")
  Test.equal(menu.widgetsById.package_restore.definition.text, "Restore Manager…")
  Test.falsy(menu.widgetsById.package_enable_disable)
  Test.falsy(menu.widgetsById.package_uninstall)
  Test.contains(
    menu.widgetsById.package_update_error.definition.text,
    "Could not check the official release."
  )
  Test.contains(
    menu.widgetsById.package_recovery.definition.text,
    "/state/self-update/recovery.aseprite-extension"
  )

  menu.widgetsById.package_update_action.definition.onclick()
  Test.equal(updated, 1)
  menu.widgetsById.package_restore.definition.onclick()
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

Test.case("GitHub install prompt mentions public and private repositories", function()
  local app = Fakes.app {
    allFilesExist = true,
  }
  local Dialog, dialogs = Fakes.dialog_factory()
  local ui = Ui.new({
    app = app,
    plugin = Fakes.plugin(),
    Dialog = Dialog,
  }, Model.new(1))

  ui:prompt_github_url()
  Test.equal(
    dialogs[1].widgetsById.github_prompt_source.definition.text,
    "Enter a public or private GitHub repository URL or"
  )
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
