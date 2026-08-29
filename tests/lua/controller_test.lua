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
  local controller, _, _, _, rpc, ui = controller_fixture()
  Test.truthy(controller:open())
  Test.truthy(ui.opened)
  Test.equal(rpc.requests[1].method, "scanInstalled")

  rpc:respond(1, {
    packages = {
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
    packages = {},
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
  Test.equal(#controller.model.updateErrors, 1)
  Test.falsy(controller.model.busy)
  Test.contains(controller.model.status, "1 installed")
  Test.contains(controller.model.status, "1 update source unavailable")
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

Test.case("startup network failure is silent", function()
  local controller, _, calls, _, rpc, ui, timers = controller_fixture {
    now = 200000,
    preferences = {
      onboardingVersion = 1,
      startupChecks = true,
      lastStartupCheckAt = 0,
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
  Test.equal(#calls.alerts, 1)
  Test.contains(calls.alerts[1].text, "Choose Extensions")
  Test.equal(#calls.options, 1)
  Test.equal(rpc.requests[1].method, "scanInstalled")
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
