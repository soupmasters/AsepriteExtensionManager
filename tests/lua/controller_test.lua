local Test = require("testlib")
local Fakes = require("fakes")
local Controller = require("aem.controller")

local function controller_fixture(options)
  options = options or {}
  local app, calls = Fakes.app {
    apiVersion = options.apiVersion or 35,
    version = options.version or "1.3.15",
    isUIAvailable = options.isUIAvailable,
    allFilesExist = true,
    optionsResult = options.optionsResult,
  }
  local plugin = Fakes.plugin {
    preferences = options.preferences or {
      onboardingVersion = 1,
    },
  }
  local rpc = Fakes.rpc()
  local ui = Fakes.ui {
    onboardingResult = options.onboardingResult,
    githubUrl = options.githubUrl,
    packageJson = options.packageJson,
    assetSelection = options.assetSelection,
    confirms = options.confirms,
  }
  local Timer, timers = Fakes.timer_factory()
  local now = options.now or 100000
  local controller = Controller.new {
    app = app,
    plugin = plugin,
    rpc = rpc,
    ui = ui,
    Timer = Timer,
    os = {
      time = function()
        return now
      end,
    },
    clock = function()
      return now
    end,
  }
  return controller, app, calls, plugin, rpc, ui, timers
end

local function manager_update_result(version)
  version = version or "0.2.0"
  return {
    packages = {
      {
        name = "aseprite-extension-manager",
        displayName = "Aseprite Extension Manager",
        version = "0.1.0",
        isSelf = true,
        managed = false,
        update = {
          kind = "self",
          version = version,
        },
      },
    },
    updates = {
      {
        packageName = "aseprite-extension-manager",
        update = {
          kind = "self",
          version = version,
        },
      },
    },
    updateErrors = {},
  }
end

Test.case("batch mode performs no UI or helper work", function()
  local controller, _, _, _, rpc, ui = controller_fixture {
    isUIAvailable = false,
  }
  local opened, reason = controller:open()
  Test.falsy(opened)
  Test.equal(reason, "ui_unavailable")
  Test.falsy(controller:schedule_startup_check())
  Test.equal(#rpc.requests, 0)
  Test.falsy(ui.opened)
end)

Test.case("diagnostics are requested through the helper", function()
  local controller, _, _, _, rpc = controller_fixture()
  local received_result
  local received_error
  local ticket = controller:request_diagnostics(function(result, error_value)
    received_result = result
    received_error = error_value
  end)

  Test.truthy(ticket)
  Test.equal(rpc.requests[1].method, "diagnostics")
  Test.equal(type(rpc.requests[1].params), "table")
  rpc:respond(1, {
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
  Test.truthy(received_result.tools.git.installed)
  Test.truthy(received_result.tools.gh.authenticated)
  Test.equal(received_error, nil)
end)

Test.case("closed controller ignores a pending diagnostics callback", function()
  local controller, _, _, _, rpc = controller_fixture()
  local callback_count = 0
  controller:request_diagnostics(function()
    callback_count = callback_count + 1
  end)
  controller:close()
  rpc:respond(1, {
    tools = {},
  })
  Test.equal(callback_count, 0)
end)

Test.case("GitHub repository browsing uses bounded cursor requests", function()
  local controller, _, _, _, rpc, ui = controller_fixture()
  Test.truthy(controller:search_github_repositories(" animation "))
  Test.equal(rpc.requests[1].method, "listGitHubRepositories")
  Test.equal(rpc.requests[1].params.query, "animation")
  Test.equal(rpc.requests[1].params.cursor, nil)
  Test.truthy(controller.model.githubLoading)

  rpc:respond(1, {
    repositories = {
      {
        nameWithOwner = "example/animation-tools",
        url = "https://github.com/example/animation-tools",
        isPrivate = true,
      },
    },
    totalCount = 8,
    hasNextPage = true,
    endCursor = "next==",
  })
  Test.falsy(controller.model.githubLoading)
  Test.truthy(controller.model.githubLoaded)
  Test.equal(controller.model.githubTotal, 8)
  Test.equal(controller.model.githubPageCount, 2)
  Test.equal(controller.model.githubRepositories[1].nameWithOwner, "example/animation-tools")

  Test.truthy(controller:move_github_page(1))
  Test.equal(rpc.requests[2].method, "listGitHubRepositories")
  Test.equal(rpc.requests[2].params.query, "animation")
  Test.equal(rpc.requests[2].params.cursor, "next==")
  rpc:respond(2, {
    repositories = {},
    totalCount = 8,
    hasNextPage = false,
  })
  Test.equal(controller.model.githubPage, 2)
  Test.truthy(controller:move_github_page(-1))
  Test.equal(rpc.requests[3].params.cursor, nil)

  rpc:fail(3, {
    code = "GITHUB_CLI_AUTH_REQUIRED",
    message = "Run gh auth login",
  })
  Test.equal(controller.model.githubError.code, "GITHUB_CLI_AUTH_REQUIRED")
  Test.truthy(ui.refreshCount >= 6)
end)

Test.case("GitHub repository install reuses the validated URL flow", function()
  local controller, _, _, _, rpc, ui = controller_fixture()
  local repository = {
    nameWithOwner = "example/private-extension",
    url = "https://github.com/example/private-extension",
    isPrivate = true,
  }
  Test.truthy(controller:install_github_repository(repository))
  Test.equal(rpc.requests[1].method, "resolveGitHub")
  Test.equal(rpc.requests[1].params.url, repository.url)

  local blocked, _, _, _, blocked_rpc, blocked_ui = controller_fixture()
  Test.falsy(blocked:install_github_repository({}))
  Test.equal(#blocked_rpc.requests, 0)
  Test.equal(blocked_ui.errors[1].title, "GitHub Repository Unavailable")
  Test.equal(#ui.errors, 0)
end)

Test.case("incompatible versions show a message before helper launch", function()
  local controller, _, _, _, rpc, ui = controller_fixture {
    apiVersion = 34,
    version = "1.3.14",
  }
  local opened, reason = controller:open()
  Test.falsy(opened)
  Test.equal(reason, "incompatible")
  Test.equal(#rpc.requests, 0)
  Test.equal(#ui.compatibility, 1)
end)

Test.case("first-run cancellation does not launch the helper", function()
  local controller, _, _, plugin, rpc = controller_fixture {
    preferences = {},
    onboardingResult = false,
  }
  local opened, reason = controller:open()
  Test.falsy(opened)
  Test.equal(reason, "onboarding_cancelled")
  Test.equal(plugin.preferences.onboardingVersion, nil)
  Test.equal(#rpc.requests, 0)
end)

Test.case("opening manager scans catalog and attaches update state", function()
  local controller, _, _, plugin, rpc, ui = controller_fixture()
  Test.truthy(controller:open())
  Test.truthy(ui.opened)
  Test.equal(rpc.requests[1].method, "scanInstalled")

  rpc:respond(1, {
    packages = {
      {
        name = "aseprite-extension-manager",
        isSelf = true,
        version = "0.1.0",
        managed = false,
      },
      {
        name = "one",
        version = "1.0.0",
        managed = false,
      },
    },
  })
  Test.equal(rpc.requests[2].method, "refreshRegistry")
  rpc:respond(2, {
    status = "current",
    packages = {
      {
        id = "aseprite-extension-manager",
        manifestName = "aseprite-extension-manager",
        displayName = "Aseprite Extension Manager",
      },
      {
        id = "catalog-one",
        manifestName = "catalog-one",
        displayName = "Catalog One",
        releases = {},
      },
    },
    expired = false,
    fromCache = false,
  })
  Test.equal(rpc.requests[3].method, "listUpdates")
  Test.equal(rpc.requests[3].params.asepriteVersion, "1.3.15")
  Test.equal(rpc.requests[3].params.apiVersion, 35)
  Test.truthy(controller.model.busy)
  rpc:respond(3, {
    packages = {
      {
        name = "aseprite-extension-manager",
        isSelf = true,
        version = "0.1.0",
        managed = false,
        rollbackAvailable = true,
        update = {
          kind = "self",
          version = "0.2.0",
        },
      },
      {
        name = "one",
        version = "1.0.0",
        managed = true,
        update = {
          version = "1.1.0",
        },
      },
    },
    updates = {
      {
        packageName = "one",
        update = {
          version = "1.1.0",
        },
      },
    },
    updateErrors = {
      {
        packageName = "aseprite-extension-manager",
        error = {
          code = "network",
          message = "Manager recovery information.",
        },
      },
      {
        packageName = "one",
        error = {
          code = "network",
          message = "Update source is temporarily unavailable.",
        },
      },
    },
  })
  Test.equal(#controller.model.installed, 1)
  Test.equal(controller.model.installed[1].update.version, "1.1.0")
  Test.equal(controller.model.managerPackage.update.kind, "self")
  Test.equal(plugin.preferences.deferredManagerUpdateVersion, "0.2.0")
  Test.truthy(controller.model.managerPackage.rollbackAvailable)
  Test.equal(#controller.model.catalog, 1)
  Test.equal(controller.model.catalog[1].id, "catalog-one")
  Test.equal(controller.model.managerCatalogPackage.id, "aseprite-extension-manager")
  Test.equal(#controller.model.updateErrors, 1)
  Test.falsy(controller.model.busy)
  Test.contains(controller.model.status, "1 installed")
  Test.contains(controller.model.status, "1 update source unavailable")
end)

Test.case("refresh failure cannot erase a deferred manager update", function()
  local preferences = {
    onboardingVersion = 1,
    startupChecks = true,
    deferredManagerUpdateVersion = "0.2.0",
  }
  local controller, _, _, plugin, rpc = controller_fixture {
    preferences = preferences,
  }
  Test.truthy(controller:open())
  rpc:respond(1, {
    packages = {
      {
        name = "aseprite-extension-manager",
        version = "0.1.0",
        isSelf = true,
        managed = false,
      },
    },
  })
  Test.equal(controller.model.managerPackage.update.version, "0.2.0")
  Test.truthy(controller.model.managerPackage.update.deferred)

  rpc:respond(2, {
    status = "current",
    packages = {},
    expired = false,
    fromCache = false,
  })
  rpc:fail(3, {
    code = "network",
    message = "offline",
  })

  Test.equal(plugin.preferences.deferredManagerUpdateVersion, "0.2.0")
  Test.equal(controller.model.managerPackage.update.version, "0.2.0")
  Test.truthy(controller.model.managerPackage.update.deferred)
end)

Test.case("startup checks run at most daily and notify for eight seconds", function()
  local now = 200000
  local controller, _, calls, plugin, rpc, _, timers = controller_fixture {
    now = now,
    preferences = {
      onboardingVersion = 1,
      startupChecks = true,
      lastStartupCheckAt = now - 90000,
    },
  }
  Test.truthy(controller:schedule_startup_check())
  Test.equal(#timers, 1)
  timers[1]:fire()
  Test.equal(rpc.requests[1].method, "listUpdates")
  Test.equal(rpc.requests[1].params.asepriteVersion, "1.3.15")
  Test.equal(rpc.requests[1].params.apiVersion, 35)
  Test.equal(plugin.preferences.lastStartupCheckAt, now)
  rpc:respond(1, {
    updates = {
      {
        name = "one",
      },
      {
        name = "two",
      },
    },
  })
  Test.equal(#calls.tips, 1)
  Test.equal(calls.tips[1].duration, 8)

  local later_controller, _, _, _, later_rpc = controller_fixture {
    now = now + 60,
    preferences = plugin.preferences,
  }
  Test.falsy(later_controller:schedule_startup_check())
  Test.equal(#later_rpc.requests, 0)
end)

Test.case("declining a startup manager update keeps a persistent update action", function()
  local now = 200000
  local preferences = {
    onboardingVersion = 1,
    startupChecks = true,
    lastStartupCheckAt = 0,
  }
  local controller, _, calls, plugin, rpc, ui, timers = controller_fixture {
    now = now,
    preferences = preferences,
    confirms = {
      false,
    },
  }

  Test.truthy(controller:schedule_startup_check())
  timers[1]:fire()
  rpc:respond(1, manager_update_result())

  Test.equal(#ui.confirmations, 1)
  Test.equal(ui.confirmations[1].title, "Update Extension Manager")
  Test.equal(ui.confirmations[1].confirmText, "Update Now")
  Test.equal(ui.confirmations[1].cancelText, "Later")
  Test.contains(ui.confirmations[1].message, "v0.2.0")
  Test.contains(ui.confirmations[1].message, "v0.1.0")
  Test.equal(#rpc.requests, 1)
  Test.equal(rpc.shutdownCount, 1)
  Test.equal(#calls.tips, 0, "manager update must not produce a duplicate generic tip")
  Test.equal(plugin.preferences.deferredManagerUpdateVersion, "0.2.0")
  Test.equal(controller.model.managerPackage.update.kind, "self")
  Test.equal(controller.model.managerPackage.update.version, "0.2.0")

  local later_controller, _, _, _, later_rpc = controller_fixture {
    now = now + 60,
    preferences = preferences,
  }
  Test.falsy(later_controller:schedule_startup_check())
  Test.equal(#later_rpc.requests, 0)
  Test.equal(later_controller.model.managerPackage.update.kind, "self")
  Test.equal(later_controller.model.managerPackage.update.version, "0.2.0")
  Test.truthy(later_controller.model.managerPackage.update.deferred)
end)

Test.case("accepting the startup manager prompt begins one dedicated update", function()
  local controller, _, _, _, rpc, ui, timers = controller_fixture {
    now = 200000,
    preferences = {
      onboardingVersion = 1,
      startupChecks = true,
      lastStartupCheckAt = 0,
    },
    confirms = {
      true,
    },
  }

  controller:schedule_startup_check()
  timers[1]:fire()
  rpc:respond(1, manager_update_result())

  Test.equal(#ui.confirmations, 1)
  Test.equal(#rpc.requests, 2)
  Test.equal(rpc.requests[2].method, "prepareSelfUpdate")
  Test.equal(next(rpc.requests[2].params), nil)
  Test.equal(rpc.shutdownCount, 0, "helper must remain available while update prepares")
end)

Test.case("authoritative no-update startup result clears a deferred manager update", function()
  local preferences = {
    onboardingVersion = 1,
    startupChecks = true,
    lastStartupCheckAt = 0,
    deferredManagerUpdateVersion = "0.2.0",
  }
  local controller, _, _, plugin, rpc, ui, timers = controller_fixture {
    now = 200000,
    preferences = preferences,
  }
  Test.truthy(controller.model.managerPackage.update.deferred)

  controller:schedule_startup_check()
  timers[1]:fire()
  rpc:respond(1, {
    packages = {
      {
        name = "aseprite-extension-manager",
        version = "0.2.0",
        isSelf = true,
        managed = false,
      },
    },
    updates = {},
    updateErrors = {},
  })

  Test.equal(plugin.preferences.deferredManagerUpdateVersion, nil)
  Test.equal(controller.model.managerPackage.update, nil)
  Test.equal(#ui.confirmations, 0)
end)

Test.case("busy manager refresh defers the startup prompt but keeps its arrow", function()
  local controller, _, _, plugin, rpc, ui, timers = controller_fixture {
    now = 200000,
    preferences = {
      onboardingVersion = 1,
      startupChecks = true,
      lastStartupCheckAt = 0,
    },
    confirms = {
      true,
    },
  }
  controller.model.busy = true
  ui.dialog = {}

  controller:schedule_startup_check()
  timers[1]:fire()
  rpc:respond(1, manager_update_result())

  Test.equal(#ui.confirmations, 0)
  Test.equal(#rpc.requests, 1)
  Test.equal(rpc.shutdownCount, 0)
  Test.equal(plugin.preferences.deferredManagerUpdateVersion, "0.2.0")
  Test.equal(controller.model.managerPackage.update.version, "0.2.0")
  Test.equal(#timers, 2)
  Test.truthy(timers[2].started)

  timers[2]:fire()
  Test.equal(#ui.confirmations, 0, "the offer must wait while refresh is busy")
  Test.falsy(timers[2].stopped)

  controller.model.busy = false
  timers[2]:fire()
  Test.equal(#ui.confirmations, 1)
  Test.equal(ui.confirmations[1].confirmText, "Update Now")
  Test.equal(ui.confirmations[1].cancelText, "Later")
  Test.equal(rpc.requests[2].method, "prepareSelfUpdate")
  Test.equal(controller.pendingStartupManagerUpdateVersion, nil)
  Test.truthy(timers[2].stopped)
end)

Test.case("late startup results cannot prompt after controller shutdown", function()
  local controller, _, _, plugin, rpc, ui, timers = controller_fixture {
    now = 200000,
    preferences = {
      onboardingVersion = 1,
      startupChecks = true,
      lastStartupCheckAt = 0,
    },
    confirms = {
      true,
    },
  }

  controller:schedule_startup_check()
  timers[1]:fire()
  controller:close()
  rpc:respond(1, manager_update_result())

  Test.equal(#ui.confirmations, 0)
  Test.equal(#rpc.requests, 1)
  Test.equal(plugin.preferences.deferredManagerUpdateVersion, nil)
  Test.equal(controller.pendingStartupManagerUpdateVersion, nil)
end)

Test.case("startup network failure is silent", function()
  local controller, _, calls, plugin, rpc, ui, timers = controller_fixture {
    now = 200000,
    preferences = {
      onboardingVersion = 1,
      startupChecks = true,
      lastStartupCheckAt = 0,
      deferredManagerUpdateVersion = "0.2.0",
    },
  }
  controller:schedule_startup_check()
  timers[1]:fire()
  rpc:fail(1, {
    code = "network",
    message = "offline",
    retryable = true,
  })
  Test.equal(#calls.tips, 0)
  Test.equal(#ui.errors, 0)
  Test.equal(plugin.preferences.deferredManagerUpdateVersion, "0.2.0")
  Test.equal(controller.model.managerPackage.update.version, "0.2.0")
end)

Test.case("GitHub operation cancellation stops the pending request", function()
  local controller, _, _, _, rpc, ui = controller_fixture {
    githubUrl = "https://github.com/example/package",
  }
  Test.truthy(controller:install_from_github())
  Test.equal(rpc.requests[1].method, "resolveGitHub")
  Test.truthy(controller.activeOperation)
  ui.progressDialogs[1]:cancel()
  Test.truthy(rpc.requests[1].cancelled)
  Test.equal(controller.activeOperation, nil)
  Test.falsy(controller.model.busy)
end)

Test.case("missing GitHub URL mentions public and private repositories", function()
  local controller, _, _, _, rpc, ui = controller_fixture {
    githubUrl = "   ",
  }
  Test.falsy(controller:install_from_github())
  Test.equal(#rpc.requests, 0)
  Test.equal(#ui.errors, 1)
  Test.contains(ui.errors[1].error.message, "public or private GitHub")
end)

Test.case("prepared GitHub packages use native install and post-install verification", function()
  local controller, _, calls, _, rpc = controller_fixture {
    githubUrl = "https://github.com/example/package",
  }
  controller:install_from_github()
  rpc:respond(1, {
    artifactPath = "/cache/example.aseprite-extension",
    name = "example",
    version = "1.2.3",
    source = {
      kind = "github-release",
      asset = "asset-1",
    },
  })
  Test.equal(#calls.options, 1)
  Test.equal(
    calls.options[1].installExtension,
    "/cache/example.aseprite-extension"
  )
  Test.equal(rpc.requests[2].method, "verifyInstall")
  Test.equal(rpc.requests[2].params.name, "example")
  rpc:respond(2, {
    verified = true,
    receipt = {},
  })
  Test.equal(calls.tips[#calls.tips].duration, 3)
  Test.equal(rpc.requests[3].method, "scanInstalled")
end)

Test.case("local sync always selects package.json and reinstalls through Aseprite", function()
  local controller, _, calls, _, rpc = controller_fixture {
    packageJson = "/work/my-extension/package.json",
  }
  Test.truthy(controller:sync_local_folder())
  Test.equal(rpc.requests[1].method, "syncLocal")
  Test.equal(
    rpc.requests[1].params.packageJsonPath,
    "/work/my-extension/package.json"
  )
  rpc:respond(1, {
    artifactPath = "/cache/local.aseprite-extension",
    name = "local-example",
    version = "1.0.0",
    contentHash = "abc",
    source = {
      kind = "local",
    },
  })
  Test.equal(
    calls.options[1].installExtension,
    "/cache/local.aseprite-extension"
  )
end)

Test.case("an explicitly unsuccessful native install is not verified", function()
  local controller, _, _, _, rpc = controller_fixture {
    githubUrl = "https://github.com/example/package",
    optionsResult = false,
  }
  controller:install_from_github()
  rpc:respond(1, {
    artifactPath = "/cache/example.aseprite-extension",
    name = "example",
    version = "1.2.3",
    source = {
      kind = "github-release",
    },
  })

  Test.equal(rpc.requests[2].method, "scanInstalled")
  Test.contains(controller.model.status, "Scanning installed")
  for _, request in ipairs(rpc.requests) do
    Test.truthy(request.method ~= "verifyInstall")
    Test.truthy(request.method ~= "prepareRollback")
  end
end)

Test.case("native extension preferences are followed by a rescan", function()
  local controller, _, calls, _, rpc = controller_fixture()
  Test.truthy(controller:open_native_extension_preferences())
  Test.equal(#calls.alerts, 0)
  Test.equal(#calls.options, 1)
  Test.equal(rpc.requests[1].method, "scanInstalled")
end)

Test.case("failed native extension preferences do not report success", function()
  local controller, _, _, _, rpc, ui = controller_fixture {
    optionsResult = false,
  }
  Test.falsy(controller:open_native_extension_preferences("enable_disable", {
    name = "example",
  }))
  Test.equal(#ui.errors, 1)
  Test.equal(ui.errors[1].error.code, "preferences_failed")
  Test.equal(#rpc.requests, 0)
end)

Test.case("uninstall immediately removes the exact scanned package through the helper", function()
  local controller, _, calls, _, rpc, ui = controller_fixture()
  local package = {
    name = "animation-list",
    displayName = "Animation List",
    version = "2.0.0",
    path = "/profile/extensions/Animation List",
    managed = false,
  }
  controller.model:set_installed({ package })

  Test.truthy(controller:uninstall_package(package))
  Test.equal(#ui.confirmations, 0)
  Test.equal(#ui.progressDialogs, 0)
  Test.equal(rpc.requests[1].method, "uninstallPackage")
  Test.equal(rpc.requests[1].params.name, "animation-list")
  Test.equal(rpc.requests[1].params.version, "2.0.0")
  Test.equal(rpc.requests[1].params.path, "/profile/extensions/Animation List")
  Test.truthy(controller.restartRequired)

  rpc:respond(1, {
    name = "animation-list",
    version = "2.0.0",
    recoveryPath = "/profile/extension-manager/uninstalled/example/extension",
    restartRequired = true,
    receiptCleanupPending = false,
  })

  Test.equal(#calls.alerts, 1)
  Test.equal(calls.alerts[1].title, "Restart Aseprite")
  Test.contains(calls.alerts[1].text, "Restart Aseprite now")
  Test.contains(calls.alerts[1].text, "may remain partly active")
  Test.contains(calls.alerts[1].text, "Recovery copy")
  Test.truthy(controller.restartRequired)
  Test.equal(#controller.model.installed, 0)
  Test.contains(controller.model.status, "Restart Aseprite")
  Test.equal(#rpc.requests, 1)

  Test.falsy(controller:install_from_github())
  Test.falsy(controller:sync_local_folder())
  Test.falsy(controller:refresh(false))
  Test.falsy(controller:install_registry_package {
    id = "another",
    name = "another",
    latest = { version = "1.0.0" },
  })
  Test.falsy(controller:update_package {
    name = "another",
    managed = true,
    update = { version = "2.0.0" },
  })
  Test.falsy(controller:restore_package {
    name = "another",
    managed = true,
  })
  Test.falsy(controller:uninstall_package {
    name = "another",
    version = "1.0.0",
    path = "/profile/extensions/another",
  })
  Test.falsy(controller:open_native_extension_preferences("enable_disable", {
    name = "another",
  }))
  Test.equal(#rpc.requests, 1)
  Test.equal(#ui.confirmations, 0)
  Test.equal(#calls.options, 0)
  Test.contains(calls.tips[#calls.tips].message, "Restart Aseprite")
end)

Test.case("busy refresh prevents uninstall from racing a stale scan", function()
  local controller, _, _, _, rpc, ui = controller_fixture()
  Test.truthy(controller:refresh(false))
  Test.truthy(controller.model.busy)

  Test.falsy(controller:uninstall_package {
    name = "example",
    version = "1.0.0",
    path = "/profile/extensions/example",
  })

  Test.equal(#ui.confirmations, 0)
  Test.equal(#rpc.requests, 1)
  Test.equal(rpc.requests[1].method, "scanInstalled")
end)

Test.case("closing during uninstall preserves the restart lock", function()
  local controller, _, calls, _, rpc = controller_fixture()
  Test.truthy(controller:uninstall_package {
    name = "example",
    version = "1.0.0",
    path = "/profile/extensions/example",
  })
  Test.truthy(controller.restartRequired)
  Test.equal(controller.activeOperation.kind, "uninstall")

  controller:on_dialog_closed()

  Test.truthy(rpc.requests[1].cancelled)
  Test.equal(controller.activeOperation, nil)
  Test.truthy(controller.restartRequired)
  Test.contains(controller.model.status, "Restart Aseprite")
  Test.falsy(controller:install_from_github())
  Test.equal(#rpc.requests, 1)
  Test.contains(calls.tips[#calls.tips].message, "Restart Aseprite")
end)

Test.case("definite helper rejection clears the provisional restart lock", function()
  local controller, _, _, _, rpc, ui = controller_fixture()
  Test.truthy(controller:uninstall_package {
    name = "example",
    version = "1.0.0",
    path = "/profile/extensions/example",
  })

  rpc:fail(1, {
    code = "INSTALLED_PACKAGE_CHANGED",
    message = "Refresh and try again.",
  })

  Test.falsy(controller.restartRequired)
  Test.equal(#ui.errors, 1)
  Test.equal(ui.errors[1].error.code, "INSTALLED_PACKAGE_CHANGED")
end)

Test.case("uncertain uninstall failure keeps the restart lock", function()
  local controller, _, _, _, rpc, ui = controller_fixture()
  Test.truthy(controller:uninstall_package {
    name = "example",
    version = "1.0.0",
    path = "/profile/extensions/example",
  })

  rpc.requests[1].callbacks.onError({
    code = "connection_closed",
    message = "The helper connection closed.",
  }, false)

  Test.truthy(controller.restartRequired)
  Test.contains(controller.model.status, "Restart Aseprite")
  Test.equal(#ui.errors, 1)
end)

Test.case("manager uninstalls never reach the helper", function()
  local controller, _, _, _, rpc, ui = controller_fixture()
  Test.falsy(controller:uninstall_package {
    name = "aseprite-extension-manager",
    version = "0.1.0",
    path = "/profile/extensions/aseprite-extension-manager",
    isSelf = true,
  })
  Test.equal(#ui.confirmations, 0)
  Test.equal(#rpc.requests, 0)
end)

Test.case("manager cannot open native disable or uninstall preferences", function()
  local controller, _, calls, _, rpc = controller_fixture()
  Test.falsy(controller:open_native_extension_preferences("uninstall", {
    name = "ASEPRITE-EXTENSION-MANAGER",
    isSelf = true,
  }))
  Test.equal(#calls.alerts, 0)
  Test.equal(#calls.options, 0)
  Test.equal(#rpc.requests, 0)
end)

Test.case("verification RPC errors offer an immediate managed restore", function()
  local controller, _, calls, _, rpc, ui = controller_fixture {
    githubUrl = "https://github.com/example/package",
    confirms = {
      true,
    },
  }
  controller.model:set_installed({
    {
      name = "example",
      displayName = "Example",
      version = "1.0.0",
      managed = true,
    },
  })

  controller:install_from_github()
  rpc:respond(1, {
    artifactPath = "/cache/example-2.aseprite-extension",
    name = "example",
    version = "2.0.0",
    source = {
      kind = "github-release",
    },
  })
  Test.equal(rpc.requests[2].method, "verifyInstall")
  rpc:fail(2, {
    code = "verification_io",
    message = "Could not read the installed manifest.",
    details = {
      rollbackAvailable = true,
      currentIntact = false,
    },
  })
  Test.equal(#ui.errors, 0)
  Test.equal(rpc.requests[3].method, "prepareRollback")
  Test.equal(rpc.requests[3].params.name, "example")

  rpc:respond(3, {
    artifactPath = "/cache/example-1.aseprite-extension",
    name = "example",
    version = "1.0.0",
    source = {
      kind = "rollback",
    },
  })
  Test.equal(#calls.options, 2)
  Test.equal(
    calls.options[2].installExtension,
    "/cache/example-1.aseprite-extension"
  )
end)

Test.case("intact installation verification failure does not restore", function()
  local controller, _, _, _, rpc, ui = controller_fixture {
    githubUrl = "https://github.com/example/package",
    confirms = {
      true,
    },
  }
  controller.model:set_installed({
    {
      name = "example",
      version = "1.0.0",
      managed = true,
    },
  })

  controller:install_from_github()
  rpc:respond(1, {
    artifactPath = "/cache/example-2.aseprite-extension",
    name = "example",
    version = "2.0.0",
    source = {
      kind = "github-release",
    },
  })
  rpc:respond(2, {
    verified = false,
    currentIntact = true,
    rollbackAvailable = false,
    message = "The previous version is still installed.",
  })

  Test.equal(rpc.requests[3].method, "scanInstalled")
  Test.equal(#ui.errors, 0)
  for _, request in ipairs(rpc.requests) do
    Test.truthy(request.method ~= "prepareRollback")
  end
end)

Test.case("verification failure restores only when helper reports rollback", function()
  local controller, _, _, _, rpc, ui = controller_fixture {
    githubUrl = "https://github.com/example/package",
    confirms = {
      true,
    },
  }
  controller.model:set_installed({
    {
      name = "example",
      version = "1.0.0",
      managed = true,
    },
  })

  controller:install_from_github()
  rpc:respond(1, {
    artifactPath = "/cache/example-2.aseprite-extension",
    name = "example",
    version = "2.0.0",
    source = {
      kind = "github-release",
    },
  })
  rpc:respond(2, {
    verified = false,
    currentIntact = false,
    rollbackAvailable = true,
    message = "The updated extension is incomplete.",
  })

  Test.equal(#ui.errors, 0)
  Test.equal(rpc.requests[3].method, "prepareRollback")
  Test.equal(rpc.requests[3].params.name, "example")
end)

Test.case("update preparation forwards direct source and host compatibility", function()
  local controller, _, _, _, rpc = controller_fixture()
  local source = {
    kind = "prepared",
    resolution = {
      artifactPath = "/cache/example-2.aseprite-extension",
      immutableId = "commit:abc",
    },
  }
  Test.truthy(controller:update_package({
    name = "example",
    version = "1.0.0",
    managed = true,
    source = {
      kind = "github-repository",
    },
    update = {
      version = "2.0.0",
      source = source,
    },
  }))

  Test.equal(rpc.requests[1].method, "preparePackage")
  Test.equal(rpc.requests[1].params.source, source)
  Test.equal(rpc.requests[1].params.asepriteVersion, "1.3.15")
  Test.equal(rpc.requests[1].params.apiVersion, 35)
end)

Test.case("unmanaged manager updates use the dedicated safe self-update flow", function()
  local controller, _, calls, _, rpc, ui, timers = controller_fixture {
    confirms = {
      true,
    },
  }
  Test.truthy(controller:update_package({
    name = "aseprite-extension-manager",
    isSelf = true,
    version = "0.1.0",
    managed = false,
    update = {
      kind = "self",
      version = "0.2.0",
    },
  }))

  Test.equal(#ui.confirmations, 1)
  Test.contains(ui.confirmations[1].message, "recovery package")
  Test.contains(ui.confirmations[1].message, "restart Aseprite")
  Test.equal(rpc.requests[1].method, "prepareSelfUpdate")
  Test.equal(next(rpc.requests[1].params), nil)

  rpc:respond(1, {
    artifactPath = "/state/self-update/candidate.aseprite-extension",
    recoveryArtifact = "/state/self-update/recovery.aseprite-extension",
    name = "aseprite-extension-manager",
    version = "0.2.0",
    selfUpdate = true,
    restartRequired = true,
  })

  Test.equal(rpc.shutdownCount, 1)
  Test.equal(#timers, 1)
  Test.equal(timers[1].options.interval, 0.5)
  Test.truthy(timers[1].started)
  Test.equal(#calls.options, 0)

  timers[1]:fire()
  Test.equal(#calls.options, 1)
  Test.equal(
    calls.options[1].installExtension,
    "/state/self-update/candidate.aseprite-extension"
  )
  Test.equal(#rpc.requests, 1, "self-update must not use stale post-install verification")
  Test.equal(controller.selfUpdateTimer, nil)
  Test.equal(#calls.alerts, 1)
  Test.contains(calls.alerts[1].text, "Recovery package")
  Test.contains(calls.alerts[1].text, "Restart Aseprite")
end)

Test.case("manager self-update confirmation can cancel before creating a transaction", function()
  local controller, _, _, _, rpc, ui = controller_fixture {
    confirms = {
      false,
    },
  }
  Test.falsy(controller:update_package({
    name = "aseprite-extension-manager",
    isSelf = true,
    version = "0.1.0",
    managed = false,
    update = {
      kind = "self",
      version = "0.2.0",
    },
  }))
  Test.equal(#ui.confirmations, 1)
  Test.equal(#rpc.requests, 0)
end)

Test.case("manager self-update refuses an incomplete recovery transaction", function()
  local controller, _, calls, _, rpc, ui = controller_fixture {
    confirms = {
      true,
    },
  }
  controller:update_package({
    name = "aseprite-extension-manager",
    isSelf = true,
    version = "0.1.0",
    managed = false,
    update = {
      kind = "self",
      version = "0.2.0",
    },
  })
  rpc:respond(1, {
    artifactPath = "/state/self-update/candidate.aseprite-extension",
    name = "aseprite-extension-manager",
    version = "0.2.0",
    selfUpdate = true,
    restartRequired = true,
  })

  Test.equal(rpc.shutdownCount, 0)
  Test.equal(#calls.options, 0)
  Test.equal(#ui.errors, 1)
  Test.equal(ui.errors[1].error.code, "invalid_self_update_artifact")
end)

Test.case("manager restore uses the dedicated self-rollback flow", function()
  local controller, _, calls, _, rpc, ui, timers = controller_fixture {
    confirms = {
      true,
    },
  }
  Test.truthy(controller:restore_package({
    name = "aseprite-extension-manager",
    isSelf = true,
    version = "0.2.0",
    rollbackAvailable = true,
  }))
  Test.equal(rpc.requests[1].method, "prepareSelfRollback")
  Test.equal(next(rpc.requests[1].params), nil)
  Test.contains(ui.confirmations[1].message, "restart Aseprite")

  rpc:respond(1, {
    artifactPath = "/state/self-update/candidate.aseprite-extension",
    recoveryArtifact = "/state/self-update/recovery.aseprite-extension",
    name = "aseprite-extension-manager",
    version = "0.1.0",
    selfUpdate = true,
    restartRequired = true,
  })
  timers[1]:fire()
  Test.equal(#calls.options, 1)
  Test.equal(#rpc.requests, 1)
  Test.contains(controller.model.status, "restore")
end)

Test.case("closing the controller stops a pending self-update install timer", function()
  local controller, _, calls, _, rpc, _, timers = controller_fixture {
    confirms = {
      true,
    },
  }
  controller:update_package({
    name = "aseprite-extension-manager",
    isSelf = true,
    version = "0.1.0",
    managed = false,
    update = {
      kind = "self",
      version = "0.2.0",
    },
  })
  rpc:respond(1, {
    artifactPath = "/state/self-update/candidate.aseprite-extension",
    recoveryArtifact = "/state/self-update/recovery.aseprite-extension",
    name = "aseprite-extension-manager",
    version = "0.2.0",
    selfUpdate = true,
    restartRequired = true,
  })
  controller:close()
  Test.truthy(timers[1].stopped)
  Test.equal(controller.selfUpdateTimer, nil)
  timers[1]:fire()
  Test.equal(#calls.options, 0)
end)

Test.case("catalog preparation forwards host compatibility", function()
  local controller, _, _, _, rpc = controller_fixture()
  Test.truthy(controller:install_registry_package({
    id = "example",
    name = "example",
    version = "2.0.0",
  }))

  Test.equal(rpc.requests[1].method, "preparePackage")
  Test.equal(rpc.requests[1].params.packageId, "example")
  Test.equal(rpc.requests[1].params.version, "2.0.0")
  Test.equal(rpc.requests[1].params.asepriteVersion, "1.3.15")
  Test.equal(rpc.requests[1].params.apiVersion, 35)
end)

Test.case("manager catalog entry cannot use the generic install flow", function()
  local controller, _, _, _, rpc = controller_fixture()
  Test.falsy(controller:install_registry_package({
    id = "ASEPRITE-EXTENSION-MANAGER",
    manifestName = "aseprite-extension-manager",
    latest = {
      version = "0.2.0",
    },
  }))
  Test.equal(#rpc.requests, 0)
end)

Test.case("controller close cancels active work and shuts down the helper", function()
  local controller, _, _, _, rpc, ui = controller_fixture {
    githubUrl = "https://github.com/example/package",
  }
  controller:install_from_github()
  Test.truthy(controller.activeOperation)
  controller:close()
  Test.equal(controller.activeOperation, nil)
  Test.truthy(rpc.requests[1].cancelled)
  Test.equal(rpc.shutdownCount, 1)
  Test.falsy(ui.opened)
end)
