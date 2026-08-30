local Compat = require("aem.compat")
local Model = require("aem.model")
local Rpc = require("aem.rpc")
local Ui = require("aem.ui")

local Controller = {}
Controller.__index = Controller

local STARTUP_INTERVAL_SECONDS = 24 * 60 * 60
local SELF_UPDATE_DELAY_SECONDS = 0.5

local function list_from(result, primary, secondary)
  if type(result) ~= "table" then
    return {}
  end
  local value = result[primary]
  if type(value) ~= "table" and secondary then
    value = result[secondary]
  end
  return type(value) == "table" and value or {}
end

local function valid_artifact(artifact)
  return type(artifact) == "table"
    and type(artifact.artifactPath) == "string"
    and artifact.artifactPath ~= ""
    and type(artifact.name) == "string"
    and artifact.name ~= ""
    and artifact.version ~= nil
end

local function requested_version(package)
  if package.version then
    return package.version
  end
  if package.latest and package.latest.version then
    return package.latest.version
  end
  return nil
end

local function is_manager_package(package)
  return Model.is_manager_installed_package(package)
end

local function is_self_update(package)
  return is_manager_package(package)
    and type(package.update) == "table"
    and package.update.kind == "self"
end

local function manager_update_version(package)
  if not is_self_update(package) then
    return nil
  end
  local version = package.update.version
  if type(version) ~= "string" or not version:match("^%d+%.%d+%.%d+$") then
    return nil
  end
  return version
end

local function find_manager_package(packages)
  for _, package in ipairs(packages or {}) do
    if is_manager_package(package) then
      return package
    end
  end
  return nil
end

local function is_manager_update_entry(entry)
  if type(entry) ~= "table" then
    return false
  end
  if is_self_update(entry) then
    return true
  end
  return Model.is_manager_name(entry.packageName or entry.name)
    and type(entry.update) == "table"
    and entry.update.kind == "self"
end

function Controller.new(environment)
  local self = setmetatable({
    environment = environment,
    app = environment.app,
    plugin = environment.plugin,
    model = Model.new(6, 5),
    rpc = environment.rpc,
    startupTimer = nil,
    startupOfferTimer = nil,
    pendingStartupManagerUpdateVersion = nil,
    selfUpdateTimer = nil,
    activeOperation = nil,
    restartRequired = false,
    refreshGeneration = 0,
    githubGeneration = 0,
    githubTicket = nil,
    githubCursors = {},
    closed = false,
  }, Controller)

  if environment.ui then
    self.ui = environment.ui
  else
    self.ui = Ui.new(environment, self.model)
  end

  local preferences = self.plugin.preferences
  if preferences.startupChecks == nil then
    preferences.startupChecks = true
  end
  self:_restore_pending_manager_update()
  return self
end

function Controller:_now()
  if self.environment.clock then
    return self.environment.clock()
  end
  return self.environment.os.time()
end

function Controller:_compatible()
  return Compat.check(self.app)
end

function Controller:_pending_manager_update_version()
  local preferences = self.plugin.preferences
  local version = preferences.deferredManagerUpdateVersion
  if type(version) == "string"
    and version:match("^%d+%.%d+%.%d+$")
    and Compat.compare_versions(tostring(self.plugin.version or "0.0.0"), version) < 0
  then
    return version
  end
  preferences.deferredManagerUpdateVersion = nil
  return nil
end

function Controller:_restore_pending_manager_update()
  local version = self:_pending_manager_update_version()
  if not version then
    return self.model.managerPackage
  end

  local manager = self.model.managerPackage
  if not manager then
    manager = {
      name = Model.MANAGER_NAME,
      displayName = "Aseprite Extension Manager",
      version = tostring(self.plugin.version or "0.0.0"),
      isSelf = true,
      managed = false,
    }
    self.model.managerPackage = manager
  end
  if not is_self_update(manager) then
    manager.update = {
      kind = "self",
      version = version,
      deferred = true,
    }
  end
  return manager
end

function Controller:_apply_manager_update_check(manager)
  if not manager then
    return self:_restore_pending_manager_update()
  end

  self.model.managerPackage = manager
  local version = manager_update_version(manager)
  if version
    and Compat.compare_versions(tostring(self.plugin.version or "0.0.0"), version) < 0
  then
    self.plugin.preferences.deferredManagerUpdateVersion = version
    return manager
  end

  if manager.updateError then
    return self:_restore_pending_manager_update()
  end
  self.plugin.preferences.deferredManagerUpdateVersion = nil
  if type(manager.update) == "table" and manager.update.kind == "self" then
    manager.update = nil
  end
  return manager
end

function Controller:_compatibility_params()
  return {
    asepriteVersion = tostring(self.app.version),
    apiVersion = tonumber(self.app.apiVersion),
  }
end

function Controller:_catalog_packages(packages)
  for _, package in ipairs(packages) do
    package.latest = Compat.select_release(package, self.app)
  end
  return packages
end

function Controller:_ensure_rpc()
  if not self.rpc then
    local factory = self.environment.rpcFactory
    self.rpc = factory and factory(self.environment) or Rpc.new(self.environment)
  end
  return self.rpc
end

function Controller:_restart_blocks_management()
  if not self.restartRequired then
    return false
  end
  self.app.tip("Restart Aseprite before managing more extensions.", 4)
  return true
end

function Controller:request_diagnostics(callback)
  if self.closed or type(callback) ~= "function" then
    return nil
  end
  return self:_ensure_rpc():request("diagnostics", {}, {
    onSuccess = function(result)
      if not self.closed then
        callback(result, nil)
      end
    end,
    onError = function(error_value)
      if not self.closed then
        callback(nil, error_value)
      end
    end,
  })
end

function Controller:_cancel_github_request()
  self.githubGeneration = self.githubGeneration + 1
  if self.githubTicket then
    self.githubTicket.cancel()
    self.githubTicket = nil
  end
  self.model.githubLoading = false
end

function Controller:load_github_repositories(query, cursor, page)
  if self.closed then
    return false
  end
  query = tostring(query or ""):match("^%s*(.-)%s*$")
  page = math.max(1, tonumber(page) or 1)
  self:_cancel_github_request()
  local generation = self.githubGeneration
  self.model.githubLoading = true
  self.model.githubError = nil
  self.ui:refresh()

  local params = {}
  if query ~= "" then
    params.query = query
  end
  if type(cursor) == "string" and cursor ~= "" then
    params.cursor = cursor
  end
  local ticket
  ticket = self:_ensure_rpc():request("listGitHubRepositories", params, {
    onSuccess = function(result)
      if self.closed or generation ~= self.githubGeneration then
        return
      end
      self.githubTicket = nil
      local repositories = list_from(result, "repositories")
      self.model:set_github_page(
        repositories,
        page,
        result.totalCount,
        result.hasNextPage,
        result.endCursor
      )
      if result.hasNextPage == true and type(result.endCursor) == "string" then
        self.githubCursors[page + 1] = result.endCursor
      else
        self.githubCursors[page + 1] = nil
      end
      self.ui:refresh()
    end,
    onError = function(error_value)
      if self.closed or generation ~= self.githubGeneration then
        return
      end
      self.githubTicket = nil
      self.model:set_github_error(error_value)
      self.ui:refresh()
    end,
  })
  self.githubTicket = ticket
  return ticket ~= nil
end

function Controller:search_github_repositories(query)
  query = tostring(query or ""):match("^%s*(.-)%s*$")
  self.githubCursors = {}
  self.model:set_search("github", query)
  return self:load_github_repositories(query, nil, 1)
end

function Controller:move_github_page(delta)
  delta = tonumber(delta) or 0
  local target = math.max(
    1,
    math.min(self.model.githubPage + delta, self.model.githubPageCount)
  )
  if target == self.model.githubPage then
    return false
  end
  local cursor = target == 1 and nil or self.githubCursors[target]
  if target > 1 and type(cursor) ~= "string" then
    return false
  end
  return self:load_github_repositories(self.model.githubSearch, cursor, target)
end

function Controller:_shutdown_rpc(on_done)
  local rpc = self.rpc
  self.rpc = nil
  if rpc then
    rpc:shutdown(on_done)
  elseif on_done then
    on_done()
  end
end

function Controller:_stop_self_update_timer()
  if self.selfUpdateTimer then
    pcall(function()
      self.selfUpdateTimer:stop()
    end)
    self.selfUpdateTimer = nil
  end
end

function Controller:_stop_startup_offer_timer()
  if self.startupOfferTimer then
    pcall(function()
      self.startupOfferTimer:stop()
    end)
    self.startupOfferTimer = nil
  end
end

function Controller:_offer_pending_startup_manager_update()
  if self.closed then
    self.pendingStartupManagerUpdateVersion = nil
    self:_stop_startup_offer_timer()
    return false, false
  end
  if not self.pendingStartupManagerUpdateVersion
    or self.model.busy
    or self.activeOperation
  then
    return false, false
  end

  local manager = self.model.managerPackage
  local version = manager_update_version(manager)
  if not version then
    self.pendingStartupManagerUpdateVersion = nil
    self:_stop_startup_offer_timer()
    return false, false
  end

  self.pendingStartupManagerUpdateVersion = nil
  self:_stop_startup_offer_timer()
  return true, self:update_package(manager) == true
end

function Controller:_schedule_pending_startup_manager_update()
  if self.closed or not self.pendingStartupManagerUpdateVersion then
    return
  end
  if not self.model.busy and not self.activeOperation then
    return
  end
  if self.startupOfferTimer or not self.environment.Timer then
    return
  end

  local timer
  timer = self.environment.Timer {
    interval = 0.1,
    ontick = function()
      if self.closed or not self.pendingStartupManagerUpdateVersion then
        self:_stop_startup_offer_timer()
        return
      end
      if self.model.busy or self.activeOperation then
        return
      end

      local _, update_started = self:_offer_pending_startup_manager_update()
      if not self.ui.dialog and not update_started then
        self:_shutdown_rpc()
      end
    end,
  }
  self.startupOfferTimer = timer
  timer:start()
end

function Controller:open()
  if not self.app.isUIAvailable then
    return false, "ui_unavailable"
  end

  local compatible, compatibility_error = self:_compatible()
  if not compatible then
    self.ui:show_compatibility(compatibility_error)
    return false, "incompatible"
  end

  local preferences = self.plugin.preferences
  if preferences.onboardingVersion ~= 1 then
    if not self.ui:show_onboarding() then
      return false, "onboarding_cancelled"
    end
    preferences.onboardingVersion = 1
  end

  self.closed = false
  self.ui:open(self)
  self:refresh(false)
  return true
end

function Controller:_startup_check_due()
  local preferences = self.plugin.preferences
  if preferences.startupChecks == false or preferences.onboardingVersion ~= 1 then
    return false
  end
  local last_check = tonumber(preferences.lastStartupCheckAt) or 0
  return self:_now() - last_check >= STARTUP_INTERVAL_SECONDS
end

function Controller:schedule_startup_check()
  if self.closed or not self.app.isUIAvailable then
    return false
  end
  local compatible = self:_compatible()
  if not compatible or not self:_startup_check_due() then
    return false
  end

  if not self.environment.Timer then
    self:run_startup_check()
    return true
  end

  local timer
  timer = self.environment.Timer {
    interval = 0.5,
    ontick = function()
      timer:stop()
      self.startupTimer = nil
      self:run_startup_check()
    end,
  }
  self.startupTimer = timer
  timer:start()
  return true
end

function Controller:run_startup_check()
  if self.closed or not self.app.isUIAvailable or not self:_startup_check_due() then
    return false
  end
  local compatible = self:_compatible()
  if not compatible then
    return false
  end

  self.plugin.preferences.lastStartupCheckAt = self:_now()
  self:_ensure_rpc():request("listUpdates", self:_compatibility_params(), {
    onSuccess = function(result)
      if self.closed then
        return
      end
      local updates = list_from(result, "updates", "packages")
      local ordinary_update_count = 0
      for _, update in ipairs(updates) do
        if not is_manager_update_entry(update) then
          ordinary_update_count = ordinary_update_count + 1
        end
      end
      if ordinary_update_count > 0 then
        local suffix = ordinary_update_count == 1 and "update is" or "updates are"
        self.app.tip(tostring(ordinary_update_count) .. " " .. suffix .. " available.", 8)
      end

      local manager = find_manager_package(list_from(result, "packages"))
      manager = self:_apply_manager_update_check(manager)
      self.model:set_update_errors(list_from(result, "updateErrors"))
      if self.ui.dialog then
        self.ui:refresh()
      end

      local update_started = false
      if manager_update_version(manager) then
        self.pendingStartupManagerUpdateVersion = manager_update_version(manager)
        _, update_started =
          self:_offer_pending_startup_manager_update()
        self:_schedule_pending_startup_manager_update()
      end
      local update_deferred = self.pendingStartupManagerUpdateVersion ~= nil
      if not self.ui.dialog
        and not update_started
        and not update_deferred
      then
        self:_shutdown_rpc()
      end
    end,
    onError = function()
      if self.closed then
        return
      end
      if not self.ui.dialog then
        self:_shutdown_rpc()
      end
    end,
  })
  return true
end

function Controller:refresh(show_errors)
  if self:_restart_blocks_management() then
    self.model.status = "Restart Aseprite before managing more extensions"
    self.ui:refresh()
    return false
  end
  if self.activeOperation then
    return false
  end

  self.refreshGeneration = self.refreshGeneration + 1
  local generation = self.refreshGeneration
  self.model:set_update_errors({})
  self.model.busy = true
  self.model.status = "Scanning installed extensions…"
  self.ui:refresh()

  local rpc = self:_ensure_rpc()
  rpc:request("scanInstalled", {}, {
    onSuccess = function(result)
      if generation ~= self.refreshGeneration then
        return
      end
      self.model:set_installed(list_from(result, "packages", "installed"))
      self:_restore_pending_manager_update()
      self.model.status = "Refreshing catalog…"
      self.ui:refresh()

      rpc:request("refreshRegistry", {}, {
        onSuccess = function(registry)
          if generation ~= self.refreshGeneration then
            return
          end
          self.model:set_catalog(
            self:_catalog_packages(list_from(registry, "packages")),
            registry.status,
            registry.expired,
            registry.fromCache
          )
          self.model.status = "Checking for updates…"
          self.ui:refresh()

          rpc:request("listUpdates", self:_compatibility_params(), {
            onSuccess = function(updates)
              if generation ~= self.refreshGeneration then
                return
              end
              self.model:set_installed(list_from(updates, "packages"))
              self:_apply_manager_update_check(self.model.managerPackage)
              self.model:set_update_errors(list_from(updates, "updateErrors"))
              self.model.busy = false
              if registry.expired then
                self.model.status =
                  "Catalog expired · direct GitHub and local sync remain available"
              else
                self.model.status =
                  "Ready · " .. tostring(#self.model.installed) .. " installed"
                if #self.model.updateErrors > 0 then
                  local suffix = #self.model.updateErrors == 1 and "source" or "sources"
                  self.model.status = self.model.status
                    .. " · "
                    .. tostring(#self.model.updateErrors)
                    .. " update "
                    .. suffix
                    .. " unavailable"
                end
              end
              self.ui:refresh()
            end,
            onError = function(error_value)
              if generation ~= self.refreshGeneration then
                return
              end
              self.model.busy = false
              if registry.expired then
                self.model.status =
                  "Catalog expired · direct GitHub and local sync remain available"
              else
                self.model.status = "Ready · update check unavailable"
              end
              self.ui:refresh()
              if show_errors then
                self.ui:show_error("Update Check Failed", error_value)
              end
            end,
          })
        end,
        onError = function(error_value)
          if generation ~= self.refreshGeneration then
            return
          end
          self.model.busy = false
          self.model.status = "Catalog unavailable · installed extensions were scanned"
          self.ui:refresh()
          if show_errors then
            self.ui:show_error("Catalog Refresh Failed", error_value)
          end
        end,
      })
    end,
    onError = function(error_value)
      if generation ~= self.refreshGeneration then
        return
      end
      self.model.busy = false
      self.model.status = "The bundled helper is unavailable"
      self.ui:refresh()
      if show_errors then
        self.ui:show_error("Extension Scan Failed", error_value)
      end
    end,
  })
  return true
end

function Controller:_begin_operation(title, message, options)
  if self:_restart_blocks_management() then
    return nil
  end
  if self.activeOperation then
    self.app.tip("Another extension operation is already running.", 2)
    return nil
  end

  local operation = {
    cancelled = false,
    ticket = nil,
  }
  self.activeOperation = operation
  self.model.busy = true
  self.model.status = message or "Working…"
  self.ui:refresh()

  if not options or options.progress ~= false then
    operation.progress = self.ui:show_progress(title, message, function()
      operation.cancelled = true
      self:_stop_self_update_timer()
      local restart_helper = operation.selfUpdatePrepared == true and not self.closed
      if self.activeOperation == operation then
        self.activeOperation = nil
        self.model.busy = false
        self.model.status = "Operation cancelled"
        self.ui:refresh()
      end
      if restart_helper then
        self:refresh(false)
      end
    end)
  end
  return operation
end

function Controller:_set_ticket(operation, ticket)
  operation.ticket = ticket
  if operation.progress then
    operation.progress:attach(ticket)
  end
end

function Controller:_update_progress(operation, event)
  if operation.cancelled or self.activeOperation ~= operation then
    return
  end
  if operation.progress then
    operation.progress:update(event)
  end
  if event and event.message then
    self.model.status = event.message
    self.ui:refresh()
  end
end

function Controller:_replace_progress(operation, title, message)
  if operation.progress then
    operation.progress:close()
  end
  operation.progress = self.ui:show_progress(title, message, function()
    operation.cancelled = true
    self:_stop_self_update_timer()
    local restart_helper = operation.selfUpdatePrepared == true and not self.closed
    if operation.ticket then
      operation.ticket.cancel()
    end
    if self.activeOperation == operation then
      self.activeOperation = nil
      self.model.busy = false
      self.model.status = "Operation cancelled"
      self.ui:refresh()
    end
    if restart_helper then
      self:refresh(false)
    end
  end)
end

function Controller:_finish_operation(operation, status)
  if self.activeOperation ~= operation then
    return
  end
  if operation.progress then
    operation.progress:close()
  end
  self.activeOperation = nil
  self.model.busy = false
  self.model.status = status or "Ready"
  self.ui:refresh()
end

function Controller:_operation_error(operation, title, error_value)
  if operation.cancelled or self.activeOperation ~= operation then
    return
  end
  self:_finish_operation(operation, "Operation failed")
  self.ui:show_error(title, error_value)
end

function Controller:_find_installed(name)
  local folded_name = tostring(name or ""):lower()
  for _, package in ipairs(self.model.installed) do
    if tostring(package.name or ""):lower() == folded_name then
      return package
    end
  end
  return nil
end

function Controller:_handle_verification_failure(
  operation,
  artifact,
  restore_context,
  error_value,
  verification
)
  if operation.cancelled or self.activeOperation ~= operation then
    return
  end

  verification = type(verification) == "table" and verification or {}
  if verification.currentIntact == true then
    self:_finish_operation(operation, "Installation cancelled or not completed")
    self:refresh(false)
    return
  end

  local previous = self:_find_installed(artifact.name)
  local can_restore = not restore_context
    and verification.rollbackAvailable == true
    and previous ~= nil
    and previous.managed == true
  self:_finish_operation(operation, "Installation could not be verified")
  if can_restore and self.ui:confirm(
    "Installation Verification Failed",
    "Aseprite did not finish installing "
      .. artifact.name
      .. ". Restore the previous cached package now?",
    "Restore"
  ) then
    self:restore_package(previous)
    return
  end

  self.ui:show_error("Installation Verification Failed", error_value)
end

function Controller:_verify_install(operation, artifact, restore_context)
  local params = {
    name = artifact.name,
    version = tostring(artifact.version),
    artifactPath = artifact.artifactPath,
    source = artifact.source or {},
  }
  local ticket
  ticket = self:_ensure_rpc():request("verifyInstall", params, {
    onSuccess = function(result)
      if operation.cancelled or self.activeOperation ~= operation then
        return
      end
      if result.verified == true then
        local action = restore_context and "Restored " or "Installed "
        self:_finish_operation(
          operation,
          action .. artifact.name .. " " .. tostring(artifact.version)
        )
        self.app.tip(
          action .. artifact.name .. " " .. tostring(artifact.version),
          3
        )
        self:refresh(false)
        return
      end

      self:_handle_verification_failure(
        operation,
        artifact,
        restore_context,
        {
          code = "verification_failed",
          message = result.message
            or "The installed extension did not match the prepared package.",
        },
        result
      )
    end,
    onError = function(error_value)
      local details = type(error_value) == "table"
          and type(error_value.details) == "table"
          and error_value.details
        or {}
      self:_handle_verification_failure(
        operation,
        artifact,
        restore_context,
        error_value,
        details
      )
    end,
    onProgress = function(event)
      self:_update_progress(operation, event)
    end,
  })
  self:_set_ticket(operation, ticket)
end

function Controller:_install_self_update(operation, artifact, restore_context)
  operation.selfUpdatePrepared = true
  self:_update_progress(operation, {
    message = "Stopping the helper before Aseprite replaces the manager…",
  })

  local function resume_after_failure()
    if self.closed then
      return
    end
    self:refresh(false)
  end

  local function install()
    if self.closed or operation.cancelled or self.activeOperation ~= operation then
      return
    end
    self:_update_progress(operation, {
      message = "Waiting for Aseprite's installation confirmation…",
    })

    local install_result
    local install_ok, install_error = pcall(function()
      install_result = self.app.command.Options {
        installExtension = artifact.artifactPath,
      }
    end)
    if not install_ok then
      self:_operation_error(operation, "Aseprite Installer Failed", {
        code = "installer_failed",
        message = tostring(install_error),
        details = {
          recoveryArtifact = artifact.recoveryArtifact,
        },
      })
      resume_after_failure()
      return
    end
    if install_result == false then
      self:_finish_operation(operation, "Manager installation cancelled")
      resume_after_failure()
      return
    end

    -- Installing this package can unload this controller before Options returns.
    -- The new helper reconciles and verifies the pending update after restart.
    if self.closed or self.activeOperation ~= operation then
      return
    end
    local action = restore_context and "restore" or "update"
    self:_finish_operation(operation, "Restart Aseprite to finish the manager " .. action)
    self.app.alert {
      title = "Restart Aseprite",
      text = "Restart Aseprite before using Extension Manager again. "
        .. "The new helper will verify the manager on startup.\n\n"
        .. "Recovery package: "
        .. tostring(artifact.recoveryArtifact or "saved by the helper"),
      buttons = "OK",
    }
  end

  self:_shutdown_rpc(function()
    if self.closed or operation.cancelled or self.activeOperation ~= operation then
      return
    end
    if not self.environment.Timer then
      install()
      return
    end

    local timer
    timer = self.environment.Timer {
      interval = SELF_UPDATE_DELAY_SECONDS,
      ontick = function()
        timer:stop()
        if self.selfUpdateTimer == timer then
          self.selfUpdateTimer = nil
        end
        install()
      end,
    }
    self.selfUpdateTimer = timer
    timer:start()
  end)
end

function Controller:_install_artifact(operation, artifact, restore_context)
  if operation.cancelled or self.activeOperation ~= operation then
    return
  end
  if not valid_artifact(artifact) then
    self:_operation_error(operation, "Package Preparation Failed", {
      code = "invalid_artifact",
      message = "The helper did not return a complete prepared package.",
    })
    return
  end
  if not self.app.fs.isFile(artifact.artifactPath) then
    self:_operation_error(operation, "Package Preparation Failed", {
      code = "artifact_missing",
      message = "The prepared extension file is missing.",
    })
    return
  end

  if artifact.selfUpdate == true then
    if not Model.is_manager_name(artifact.name)
      or artifact.restartRequired ~= true
      or type(artifact.recoveryArtifact) ~= "string"
      or artifact.recoveryArtifact == ""
      or not self.app.fs.isFile(artifact.recoveryArtifact)
    then
      self:_operation_error(operation, "Manager Update Preparation Failed", {
        code = "invalid_self_update_artifact",
        message = "The helper did not return a complete manager update and recovery package.",
      })
      return
    end
    self:_install_self_update(operation, artifact, restore_context)
    return
  end

  self:_update_progress(operation, {
    message = "Waiting for Aseprite's installation confirmation…",
  })
  local install_result
  local install_ok, install_error = pcall(function()
    install_result = self.app.command.Options {
      installExtension = artifact.artifactPath,
    }
  end)
  if not install_ok then
    self:_operation_error(operation, "Aseprite Installer Failed", {
      code = "installer_failed",
      message = tostring(install_error),
    })
    return
  end
  if install_result == false then
    self:_finish_operation(operation, "Installation cancelled or not completed")
    self:refresh(false)
    return
  end
  if operation.cancelled then
    return
  end

  self:_update_progress(operation, {
    message = "Verifying the installed extension…",
  })
  self:_verify_install(operation, artifact, restore_context)
end

function Controller:_resolved_github(operation, url, result)
  if operation.cancelled or self.activeOperation ~= operation then
    return
  end

  local choices = result.choices or result.assets
  if type(choices) == "table" and #choices > 0 then
    if operation.progress then
      operation.progress:close()
      operation.progress = nil
    end
    local selection = self.ui:choose_github_asset(choices)
    if not selection then
      operation.cancelled = true
      self.activeOperation = nil
      self.model.busy = false
      self.model.status = "Installation cancelled"
      self.ui:refresh()
      return
    end
    self:_replace_progress(operation, "Install from GitHub", "Resolving the selected release asset…")
    self:_resolve_github(operation, url, selection)
    return
  end

  if valid_artifact(result) then
    self:_install_artifact(operation, result, false)
    return
  end

  local ticket
  ticket = self:_ensure_rpc():request("preparePackage", {
    resolution = result,
  }, {
    onSuccess = function(artifact)
      self:_install_artifact(operation, artifact, false)
    end,
    onError = function(error_value)
      self:_operation_error(operation, "GitHub Package Preparation Failed", error_value)
    end,
    onProgress = function(event)
      self:_update_progress(operation, event)
    end,
  })
  self:_set_ticket(operation, ticket)
end

function Controller:_resolve_github(operation, url, selection)
  local params = {
    url = url,
  }
  if selection ~= nil then
    params.selection = selection
  end
  local ticket
  ticket = self:_ensure_rpc():request("resolveGitHub", params, {
    onSuccess = function(result)
      self:_resolved_github(operation, url, result)
    end,
    onError = function(error_value)
      self:_operation_error(operation, "GitHub Resolution Failed", error_value)
    end,
    onProgress = function(event)
      self:_update_progress(operation, event)
    end,
  })
  self:_set_ticket(operation, ticket)
end

function Controller:_start_github_install(url)
  url = tostring(url):match("^%s*(.-)%s*$")
  if url == "" then
    self.ui:show_error("GitHub URL Required", {
      code = "missing_url",
      message = "Enter a public or private GitHub repository or release asset URL.",
    })
    return false
  end

  local operation = self:_begin_operation("Install from GitHub", "Resolving the GitHub source…")
  if not operation then
    return false
  end
  self:_resolve_github(operation, url)
  return true
end

function Controller:install_from_github()
  if self:_restart_blocks_management() then
    return false
  end
  if self.activeOperation then
    return false
  end
  local url = self.ui:prompt_github_url()
  if not url then
    return false
  end
  return self:_start_github_install(url)
end

function Controller:install_github_repository(repository)
  if self:_restart_blocks_management() then
    return false
  end
  if self.activeOperation then
    return false
  end
  local url = type(repository) == "table" and repository.url or nil
  if type(url) ~= "string" or url == "" then
    self.ui:show_error("GitHub Repository Unavailable", {
      code = "missing_url",
      message = "The selected GitHub repository does not have a valid URL.",
    })
    return false
  end
  return self:_start_github_install(url)
end

function Controller:sync_local_folder()
  if self:_restart_blocks_management() then
    return false
  end
  if self.activeOperation then
    return false
  end
  local package_json = self.ui:prompt_package_json()
  if not package_json then
    return false
  end
  if self.app.fs.fileName(package_json):lower() ~= "package.json"
    or not self.app.fs.isFile(package_json)
  then
    self.ui:show_error("Invalid Local Folder", {
      code = "package_json_required",
      message = "Select the local extension folder's package.json file.",
    })
    return false
  end

  local operation = self:_begin_operation(
    "Link Local Folder",
    "Creating a safe snapshot and remembering the linked folder…"
  )
  if not operation then
    return false
  end
  local ticket
  ticket = self:_ensure_rpc():request("syncLocal", {
    packageJsonPath = package_json,
  }, {
    onSuccess = function(artifact)
      self:_install_artifact(operation, artifact, false)
    end,
    onError = function(error_value)
      self:_operation_error(operation, "Local Link Failed", error_value)
    end,
    onProgress = function(event)
      self:_update_progress(operation, event)
    end,
  })
  self:_set_ticket(operation, ticket)
  return true
end

function Controller:install_registry_package(package)
  if self:_restart_blocks_management() then
    return false
  end
  if self.model.registryExpired
    or package.yanked
    or self.activeOperation
    or Model.is_manager_catalog_package(package)
  then
    return false
  end
  local operation = self:_begin_operation("Install Extension", "Preparing the catalog package…")
  if not operation then
    return false
  end
  local ticket
  local version = requested_version(package)
  if not version then
    self.ui:show_error("Package Unavailable", {
      code = "no_compatible_release",
      message = "This package has no compatible stable release for this Aseprite version.",
    })
    return false
  end
  local params = self:_compatibility_params()
  params.packageId = package.id or package.name
  params.version = version
  ticket = self:_ensure_rpc():request("preparePackage", params, {
    onSuccess = function(artifact)
      self:_install_artifact(operation, artifact, false)
    end,
    onError = function(error_value)
      self:_operation_error(operation, "Package Preparation Failed", error_value)
    end,
    onProgress = function(event)
      self:_update_progress(operation, event)
    end,
  })
  self:_set_ticket(operation, ticket)
  return true
end

function Controller:update_package(package)
  local self_update = is_self_update(package)
  if self:_restart_blocks_management() then
    return false
  end
  if self.activeOperation or (not package.managed and not self_update) then
    return false
  end
  local update = type(package.update) == "table" and package.update or {}
  if self_update and not self.ui:confirm(
    "Update Extension Manager",
    "Aseprite Extension Manager v"
      .. tostring(update.version or "new")
      .. " is available. You currently have v"
      .. tostring(package.version or self.plugin.version or "unknown")
      .. ".\n\nThe helper will download the official stable release and save a recovery package. "
      .. "It must stop before Aseprite replaces the manager, and you must restart Aseprite afterward. "
      .. "If startup verification fails, the manager will show the recovery package path.",
    "Update Now",
    "Later"
  ) then
    return false
  end

  local operation = self:_begin_operation(
    self_update and "Update Extension Manager" or "Update Extension",
    self_update and "Preparing a verified manager update…" or "Preparing the update…"
  )
  if not operation then
    return false
  end
  local method = self_update and "prepareSelfUpdate" or "preparePackage"
  local params = {}
  if not self_update then
    params = self:_compatibility_params()
    params.packageId = package.name
    params.version = update.version
    params.source = update.source or package.source
  end
  local ticket
  ticket = self:_ensure_rpc():request(method, params, {
    onSuccess = function(artifact)
      if self_update and (type(artifact) ~= "table" or artifact.selfUpdate ~= true) then
        self:_operation_error(operation, "Manager Update Preparation Failed", {
          code = "invalid_self_update_artifact",
          message = "The helper did not prepare a dedicated manager update transaction.",
        })
        return
      end
      if not self_update then
        artifact.rollbackAvailable = true
      end
      self:_install_artifact(operation, artifact, false)
    end,
    onError = function(error_value)
      self:_operation_error(operation, "Update Preparation Failed", error_value)
    end,
    onProgress = function(event)
      self:_update_progress(operation, event)
    end,
  })
  self:_set_ticket(operation, ticket)
  return true
end

function Controller:restore_package(package)
  if self:_restart_blocks_management() then
    return false
  end
  if self.activeOperation or not package then
    return false
  end
  local self_rollback = is_manager_package(package)
  if self_rollback and not self.ui:confirm(
    "Restore Extension Manager",
    "The helper will save the current manager as a recovery package, then stop before Aseprite "
      .. "restores the previous verified version. You must restart Aseprite afterward. "
      .. "If startup verification fails, the manager will show the recovery package path.",
    "Prepare Restore"
  ) then
    return false
  end

  local operation = self:_begin_operation(
    self_rollback and "Restore Extension Manager" or "Restore Extension",
    self_rollback and "Preparing the previous verified manager…"
      or "Preparing the previous cached package…"
  )
  if not operation then
    return false
  end
  local ticket
  local method = self_rollback and "prepareSelfRollback" or "prepareRollback"
  local params = self_rollback and {} or {
    name = package.name,
  }
  ticket = self:_ensure_rpc():request(method, params, {
    onSuccess = function(artifact)
      if self_rollback and (type(artifact) ~= "table" or artifact.selfUpdate ~= true) then
        self:_operation_error(operation, "Manager Restore Preparation Failed", {
          code = "invalid_self_update_artifact",
          message = "The helper did not prepare a dedicated manager restore transaction.",
        })
        return
      end
      self:_install_artifact(operation, artifact, true)
    end,
    onError = function(error_value)
      self:_operation_error(operation, "Restore Preparation Failed", error_value)
    end,
    onProgress = function(event)
      self:_update_progress(operation, event)
    end,
  })
  self:_set_ticket(operation, ticket)
  return true
end

function Controller:clear_cache()
  if self:_restart_blocks_management() then
    return false
  end
  if self.activeOperation then
    return false
  end
  local operation = self:_begin_operation("Clear Cache", "Removing unused cached artifacts…")
  if not operation then
    return false
  end
  local ticket
  ticket = self:_ensure_rpc():request("clearCache", {
    preserveRestorePoints = true,
  }, {
    onSuccess = function()
      self:_finish_operation(operation, "Download cache cleared")
    end,
    onError = function(error_value)
      self:_operation_error(operation, "Cache Cleanup Failed", error_value)
    end,
    onProgress = function(event)
      self:_update_progress(operation, event)
    end,
  })
  self:_set_ticket(operation, ticket)
  return true
end

function Controller:uninstall_package(package)
  if self:_restart_blocks_management() then
    return false
  end
  if self.activeOperation
    or self.model.busy
    or not package
    or is_manager_package(package)
  then
    return false
  end
  local name = type(package.name) == "string" and package.name or nil
  local version = package.version ~= nil and tostring(package.version) or nil
  local path = type(package.path) == "string" and package.path or nil
  if not name or name == "" or not version or version == "" or not path or path == "" then
    self.ui:show_error("Uninstall Failed", {
      code = "invalid_installed_package",
      message = "Refresh the installed extension list and try again.",
    })
    return false
  end

  local display_name = tostring(package.displayName or name)
  local operation = self:_begin_operation(
    "Uninstall Extension",
    "Uninstalling " .. display_name .. "…",
    { progress = false }
  )
  if not operation then
    return false
  end
  operation.kind = "uninstall"
  self.refreshGeneration = self.refreshGeneration + 1
  self.restartRequired = true
  local ticket
  ticket = self:_ensure_rpc():request("uninstallPackage", {
    name = name,
    version = version,
    path = path,
  }, {
    onSuccess = function(result)
      if operation.cancelled or self.activeOperation ~= operation then
        return
      end
      if type(result) ~= "table"
        or result.name ~= name
        or tostring(result.version or "") ~= version
        or result.restartRequired ~= true
      then
        self:_operation_error(operation, "Uninstall Failed", {
          code = "invalid_uninstall_result",
          message = "The helper returned an invalid uninstall result.",
        })
        self.model.status = "Restart Aseprite before managing more extensions"
        self.ui:refresh()
        return
      end
      local remaining = {}
      for _, installed in ipairs(self.model.installed) do
        if tostring(installed.path or "") ~= path then
          remaining[#remaining + 1] = installed
        end
      end
      self.model:set_installed(remaining)
      self:_finish_operation(
        operation,
        "Restart Aseprite to finish removing " .. display_name
      )
      local notice = "Files for "
        .. display_name
        .. " were moved out of Aseprite. Restart Aseprite now. Until then, the extension "
        .. "may remain partly active or stop working. Do not install, update, restore, or "
        .. "remove extensions before restarting."
      if type(result.recoveryPath) == "string" and result.recoveryPath ~= "" then
        notice = notice .. "\n\nRecovery copy:\n" .. result.recoveryPath
      end
      if result.receiptCleanupPending == true then
        notice = notice
          .. "\n\nReceipt cleanup will finish the next time the manager starts."
      end
      self.app.alert {
        title = "Restart Aseprite",
        text = notice,
        buttons = "OK",
      }
    end,
    onError = function(error_value, helper_rejected)
      if helper_rejected == true then
        self.restartRequired = false
      end
      self:_operation_error(operation, "Uninstall Failed", error_value)
      if self.restartRequired then
        self.model.status = "Restart Aseprite before managing more extensions"
        self.ui:refresh()
      end
    end,
    onProgress = function(event)
      self:_update_progress(operation, event)
    end,
  })
  self:_set_ticket(operation, ticket)
  return true
end

function Controller:open_native_extension_preferences(_, package)
  if self:_restart_blocks_management() then
    return false
  end
  if is_manager_package(package) then
    return false
  end
  local opened, command_result = pcall(function()
    return self.app.command.Options()
  end)
  if not opened or command_result == false then
    self.ui:show_error("Aseprite Preferences Failed", {
      code = "preferences_failed",
      message = opened and "Aseprite did not open Preferences."
        or tostring(command_result),
    })
    return false
  end
  self:refresh(false)
  return true
end

function Controller:on_dialog_closed()
  self:_stop_self_update_timer()
  self:_cancel_github_request()
  self.refreshGeneration = self.refreshGeneration + 1
  if self.activeOperation then
    if self.activeOperation.kind == "uninstall" then
      self.restartRequired = true
      self.model.status = "Restart Aseprite before managing more extensions"
    end
    self.activeOperation.cancelled = true
    if self.activeOperation.ticket then
      self.activeOperation.ticket.cancel()
    end
    self.activeOperation = nil
  end
  self.model.busy = false
  self:_shutdown_rpc()
end

function Controller:close()
  self.closed = true
  self:_stop_self_update_timer()
  self:_stop_startup_offer_timer()
  self.pendingStartupManagerUpdateVersion = nil
  self:_cancel_github_request()
  if self.startupTimer then
    pcall(function()
      self.startupTimer:stop()
    end)
    self.startupTimer = nil
  end
  self.refreshGeneration = self.refreshGeneration + 1
  if self.activeOperation and self.activeOperation.ticket then
    self.activeOperation.ticket.cancel()
  end
  self.activeOperation = nil
  self.ui:close()
  self:_shutdown_rpc()
end

return Controller
