local Fakes = {}

function Fakes.json()
  local records = {}
  local next_id = 0
  local api = {}

  function api.encode(value)
    next_id = next_id + 1
    local key = "@json:" .. tostring(next_id)
    records[key] = value
    return key
  end

  function api.decode(value)
    local record = records[value]
    if record == nil then
      error("unknown encoded value: " .. tostring(value))
    end
    return record
  end

  function api.register(key, value)
    records[key] = value
    return key
  end

  function api.value(key)
    return records[key]
  end

  return api
end

function Fakes.app(options)
  options = options or {}
  local calls = {
    alerts = {},
    tips = {},
    options = {},
    launch = {},
  }
  local files = options.files or {}
  local app = {
    apiVersion = options.apiVersion or 35,
    version = options.version or "1.3.15",
    uiScale = options.uiScale or 1,
    isUIAvailable = options.isUIAvailable ~= false,
    os = options.os or {
      name = "macOS",
      macos = true,
      arm64 = true,
    },
    fs = {
      userConfigPath = options.userConfigPath or "/Users/Test User/Library/Aseprite",
    },
    command = {},
  }

  function app.fs.joinPath(...)
    local parts = {
      ...,
    }
    return table.concat(parts, "/"):gsub("/+", "/")
  end

  function app.fs.isFile(path)
    if options.allFilesExist then
      return true
    end
    return files[path] == true
  end

  function app.fs.fileName(path)
    return tostring(path):match("([^/\\]+)$") or ""
  end

  function app.command.Options(arguments)
    calls.options[#calls.options + 1] = arguments or {}
    return options.optionsResult
  end

  function app.command.Launch(arguments)
    calls.launch[#calls.launch + 1] = arguments or {}
    return true
  end

  function app.tip(message, duration)
    calls.tips[#calls.tips + 1] = {
      message = message,
      duration = duration,
    }
  end

  function app.alert(arguments)
    calls.alerts[#calls.alerts + 1] = arguments
    return 1
  end

  return app, calls
end

function Fakes.plugin(options)
  options = options or {}
  return {
    path = options.path or "/Extensions/Aseprite Extension Manager",
    version = options.version or "0.1.0",
    preferences = options.preferences or {},
  }
end

function Fakes.rpc()
  local rpc = {
    requests = {},
    shutdownCount = 0,
  }

  function rpc:request(method, params, callbacks)
    local request = {
      method = method,
      params = params,
      callbacks = callbacks,
      cancelled = false,
    }
    self.requests[#self.requests + 1] = request
    local ticket = {}
    function ticket.cancel()
      request.cancelled = true
    end
    request.ticket = ticket
    return ticket
  end

  function rpc:respond(index, result)
    self.requests[index].callbacks.onSuccess(result or {})
  end

  function rpc:fail(index, error_value)
    self.requests[index].callbacks.onError(error_value or {
      code = "test_error",
      message = "Test error",
    })
  end

  function rpc:progress(index, progress)
    self.requests[index].callbacks.onProgress(progress or {
      message = "Working",
    })
  end

  function rpc:shutdown(on_done)
    self.shutdownCount = self.shutdownCount + 1
    if on_done then
      on_done()
    end
  end

  return rpc
end

function Fakes.progress(on_cancel)
  local progress = {
    updates = {},
    closed = false,
    cancelled = false,
    ticket = nil,
  }

  function progress:update(event)
    self.updates[#self.updates + 1] = event
  end

  function progress:attach(ticket)
    self.ticket = ticket
  end

  function progress:close()
    self.closed = true
  end

  function progress:is_cancelled()
    return self.cancelled
  end

  function progress:cancel()
    self.cancelled = true
    if self.ticket then
      self.ticket.cancel()
    end
    if on_cancel then
      on_cancel()
    end
  end

  return progress
end

function Fakes.ui(options)
  options = options or {}
  local ui = {
    onboardingResult = options.onboardingResult ~= false,
    githubUrl = options.githubUrl,
    packageJson = options.packageJson,
    assetSelection = options.assetSelection,
    confirms = options.confirms or {},
    errors = {},
    confirmations = {},
    compatibility = {},
    progressDialogs = {},
    refreshCount = 0,
    opened = false,
    dialog = nil,
  }

  function ui:show_compatibility(message)
    self.compatibility[#self.compatibility + 1] = message
  end

  function ui:show_onboarding()
    return self.onboardingResult
  end

  function ui:open()
    self.opened = true
    self.dialog = {}
  end

  function ui:close()
    self.opened = false
    self.dialog = nil
  end

  function ui:refresh()
    self.refreshCount = self.refreshCount + 1
  end

  function ui:show_error(title, error_value)
    self.errors[#self.errors + 1] = {
      title = title,
      error = error_value,
    }
  end

  function ui:show_progress(title, message, on_cancel)
    local progress = Fakes.progress(on_cancel)
    progress.title = title
    progress.message = message
    self.progressDialogs[#self.progressDialogs + 1] = progress
    return progress
  end

  function ui:prompt_github_url()
    return self.githubUrl
  end

  function ui:prompt_package_json()
    return self.packageJson
  end

  function ui:choose_github_asset()
    return self.assetSelection
  end

  function ui:confirm(title, message, confirm_text, cancel_text)
    self.confirmations[#self.confirmations + 1] = {
      title = title,
      message = message,
      confirmText = confirm_text,
      cancelText = cancel_text,
    }
    if #self.confirms == 0 then
      return false
    end
    return table.remove(self.confirms, 1)
  end

  return ui
end

function Fakes.timer_factory()
  local timers = {}
  local function Timer(options)
    local timer = {
      options = options,
      started = false,
      stopped = false,
    }
    function timer:start()
      self.started = true
    end
    function timer:stop()
      self.stopped = true
    end
    function timer:fire()
      self.options.ontick()
    end
    timers[#timers + 1] = timer
    return timer
  end
  return Timer, timers
end

function Fakes.dialog_factory(factory_options)
  factory_options = factory_options or {}
  local dialogs = {}
  local function Dialog(options)
    options = options or {}
    local dialog = {
      options = options,
      widgets = {},
      widgetsById = {},
      sameRowCalls = {},
      data = {},
      bounds = {
        x = 0,
        y = 0,
        width = options.width or 744,
        height = options.height or 484,
      },
      sizeHint = {
        width = options.sizeHintWidth or 640,
        height = options.sizeHintHeight or 484,
      },
      repaintCount = 0,
      shown = nil,
      closed = false,
    }

    local function widget(kind, definition)
      definition = definition or {}
      local entry = {
        kind = kind,
        definition = definition,
      }
      dialog.widgets[#dialog.widgets + 1] = entry
      if definition.id then
        dialog.widgetsById[definition.id] = entry
        if kind == "entry" then
          dialog.data[definition.id] = definition.text or ""
        elseif kind == "check" then
          dialog.data[definition.id] = definition.selected == true
        end
      end
      return dialog
    end

    for _, kind in ipairs({
      "tab",
      "entry",
      "label",
      "button",
      "separator",
      "check",
      "combobox",
      "menuItem",
      "file",
      "canvas",
    }) do
      dialog[kind] = function(_, definition)
        return widget(kind, definition)
      end
    end

    function dialog:newrow()
      return widget("newrow", {})
    end

    function dialog:samerow(definition)
      self.sameRowCalls[#self.sameRowCalls + 1] = {
        after = self.widgets[#self.widgets],
        definition = definition or {},
      }
      self.sameRow = definition or true
      return self
    end

    function dialog:endtabs(definition)
      return widget("endtabs", definition)
    end

    function dialog:modify(definition)
      local existing = definition.id and self.widgetsById[definition.id]
      if existing then
        for key, value in pairs(definition) do
          existing.definition[key] = value
        end
      end
      return self
    end

    function dialog:show(show_options)
      self.shown = show_options or true
      if factory_options.shrinkSizeHintOnShow then
        self.sizeHint.width = factory_options.shrinkSizeHintOnShow
      end
      return self
    end

    function dialog:showMenu(show_options)
      self.shownMenu = show_options or true
      return self
    end

    function dialog:repaint()
      self.repaintCount = self.repaintCount + 1
      return self
    end

    function dialog:resize(width, height)
      self.bounds.width = width
      if height then
        self.bounds.height = height
      end
      if self.options.onresize then
        self.options.onresize()
      end
      return self
    end

    function dialog:close()
      if self.closed then
        return
      end
      self.closed = true
      if self.options.onclose then
        self.options.onclose()
      end
    end

    dialogs[#dialogs + 1] = dialog
    return dialog
  end
  return Dialog, dialogs
end

return Fakes
