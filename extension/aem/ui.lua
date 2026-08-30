local Protocol = require("aem.protocol")
local Model = require("aem.model")

local Ui = {}
Ui.__index = Ui

local REPOSITORY_URL = "https://github.com/soupmasters/AsepriteExtensionManager"
local REPOSITORY_LINK_TEXT = "View on GitHub"
local REPOSITORY_LINK_WIDTH = 120
local REPOSITORY_LINK_HEIGHT = 16
local LEFT_MOUSE_BUTTON = MouseButton and MouseButton.LEFT or 1
local POINTER_CURSOR = MouseCursor and MouseCursor.POINTER or nil
local RESIZE_DEBOUNCE_SECONDS = 0.1
local DEFAULT_MANAGER_WIDTH = 720
local DEFAULT_CONFIRM_WIDTH = 560
local CONFIRM_LINE_COLUMNS = 64
local PACKAGE_MENU_COLUMNS = 72
local MANAGER_WINDOW_MARGIN = 16
local FALLBACK_INLINE_WIDTH = 560
local WIDTH_PRESERVING_AUTOFIT = Align and Align.TOP or 0
local VIEW_OPTIONS = {
  browse = {
    filter = {
      { label = "All", value = "all" },
      { label = "Compatible", value = "compatible" },
      { label = "No Compatible Release", value = "unavailable" },
    },
    sort = {
      { label = "Name A-Z", value = "name_asc" },
      { label = "Name Z-A", value = "name_desc" },
      { label = "Newest", value = "recent" },
    },
  },
  installed = {
    filter = {
      { label = "All", value = "all" },
      { label = "Updates Available", value = "updates" },
      { label = "Managed", value = "managed" },
      { label = "Unmanaged", value = "unmanaged" },
    },
    sort = {
      { label = "Name A-Z", value = "name_asc" },
      { label = "Name Z-A", value = "name_desc" },
      { label = "Updates First", value = "updates_first" },
    },
  },
}

local function text(value, fallback)
  if value == nil or value == "" then
    return fallback or "—"
  end
  return tostring(value)
end

local function manager_version(plugin)
  local value = plugin and plugin.version
  if value == nil or tostring(value) == "" then
    return "Unknown"
  end
  return tostring(value)
end

local function installed_tool_status(tool)
  if type(tool) ~= "table" or type(tool.installed) ~= "boolean" then
    return "? Unknown"
  end
  if not tool.installed then
    return "✕ Not installed"
  end
  local status = "✓ Installed"
  if type(tool.version) == "string" and tool.version ~= "" then
    status = status .. " · " .. tool.version
  end
  return status
end

local function github_cli_status(tool)
  local status = installed_tool_status(tool)
  if type(tool) ~= "table" or tool.installed ~= true then
    return status
  end
  if tool.authenticated == true then
    return status .. " · ✓ Signed in"
  end
  if tool.authenticated == false then
    return status .. " · ✕ Sign-in unavailable"
  end
  return status .. " · ? Sign-in unknown"
end

local function fallback_link_color()
  local components = {
    r = 79,
    g = 148,
    b = 255,
    a = 255,
  }
  if Color then
    return Color(components)
  end
  return components
end

local function themed_link_color(context, active)
  local ok, value = pcall(function()
    return active and context.theme.color.link_hover or context.theme.color.link_text
  end)
  if ok and value then
    return value
  end
  return fallback_link_color()
end

local function paint_link(event, label, active)
  local context = event.context
  local measured = context:measureText(label)
  local text_width = tonumber(measured.width) or 0
  local text_height = tonumber(measured.height) or 0
  local canvas_width = tonumber(context.width) or text_width
  local canvas_height = tonumber(context.height) or text_height + 1
  local underline_y = math.min(text_height, math.max(canvas_height - 1, 0))

  context.color = themed_link_color(context, active)
  context:fillText(label, 0, 0)
  context.strokeWidth = 1
  context:beginPath()
  context:moveTo(0, underline_y)
  context:lineTo(math.min(text_width, canvas_width), underline_y)
  context:stroke()
end

local function inside_repository_link(event)
  return event
    and tonumber(event.x)
    and tonumber(event.y)
    and event.x >= 0
    and event.x < REPOSITORY_LINK_WIDTH
    and event.y >= 0
    and event.y < REPOSITORY_LINK_HEIGHT
end

local function source_label(source)
  if type(source) == "table" then
    local kind = text(source.kind, "managed")
    local identity = source.repository
      or source.url
      or source.asset
      or source.commit
      or source.packageJsonPath
      or source.path
    if identity and identity ~= "" then
      if kind == "local" then
        return "linked folder: " .. tostring(identity)
      end
      return kind .. ": " .. tostring(identity)
    end
    return kind
  end
  return text(source, "unmanaged")
end

local function self_update_available(package)
  return Model.is_manager_installed_package(package)
    and type(package.update) == "table"
    and package.update.kind == "self"
end

local function recovery_artifact(package)
  return type(package) == "table"
      and type(package.updateError) == "table"
      and type(package.updateError.details) == "table"
      and package.updateError.details.recoveryArtifact
    or nil
end

local function author_name(author)
  if type(author) == "table" then
    return text(author.name)
  end
  return text(author)
end

local function package_version(package)
  if package.version then
    return tostring(package.version)
  end
  if package.latest and package.latest.version then
    return tostring(package.latest.version)
  end
  return "—"
end

local function package_title(package)
  return text(
    package.displayName,
    text(package.name, text(package.manifestName, text(package.id, "Unnamed extension")))
  )
end

local function catalog_row(package)
  local suffix = "v" .. package_version(package)
  if package.author then
    suffix = suffix .. " · " .. author_name(package.author)
  end
  if not package.latest then
    suffix = suffix .. " · No compatible release"
  end
  return package_title(package) .. "  " .. suffix
end

local function installed_row(package)
  local parts = {
    package_title(package) .. "  v" .. package_version(package),
    package.managed and "✓" or "⚠",
  }
  if package.enabled ~= nil then
    parts[#parts + 1] = package.enabled and "Enabled" or "Disabled"
  end
  if package.update then
    local update_version = type(package.update) == "table" and package.update.version or package.update
    parts[#parts + 1] = "Update " .. text(update_version, "available")
  end
  if package.updateError then
    parts[#parts + 1] = "Update check failed"
  end
  if package.rollbackAvailable then
    parts[#parts + 1] = "Restore available"
  end
  return table.concat(parts, " · ")
end

local function narrow_catalog_row(package)
  local row = package_title(package) .. "  v" .. package_version(package)
  if package.author then
    row = row .. " · " .. author_name(package.author)
  end
  return row
end

local function narrow_installed_row(package)
  return table.concat({
    package_title(package) .. "  v" .. package_version(package),
    package.managed and "✓" or "⚠",
  }, " · ")
end

local function github_repository_row(repository)
  local parts = {
    text(repository.nameWithOwner, "Unnamed repository"),
    repository.isPrivate and "Private" or "Public",
  }
  if repository.isArchived then
    parts[#parts + 1] = "Archived"
  elseif repository.isFork then
    parts[#parts + 1] = "Fork"
  end
  if type(repository.updatedAt) == "string" and repository.updatedAt ~= "" then
    parts[#parts + 1] = "Updated " .. repository.updatedAt:sub(1, 10)
  end
  return table.concat(parts, " · ")
end

local function show_responsive(dialog, options)
  options = options or {}
  if options.autoscrollbars == nil then
    options.autoscrollbars = true
  end
  return dialog:show(options)
end

local function dialog_dimension(dialog, property, dimension)
  local ok, value = pcall(function()
    local rectangle = dialog[property]
    return rectangle and rectangle[dimension]
  end)
  if not ok then
    return nil
  end
  value = tonumber(value)
  if not value or value <= 0 then
    return nil
  end
  return value
end

local function object_dimension(object, dimension)
  local ok, value = pcall(function()
    return object and object[dimension]
  end)
  if not ok then
    return nil
  end
  value = tonumber(value)
  if not value or value <= 0 then
    return nil
  end
  return value
end

local function ui_scale(app_instance)
  return math.max(1, tonumber(app_instance and app_instance.uiScale) or 1)
end

local function utf8_length(value)
  if utf8 and type(utf8.len) == "function" then
    local ok, length = pcall(utf8.len, value)
    if ok and length then
      return length
    end
  end
  return #value
end

local function utf8_prefix(value, length)
  if utf8 and type(utf8.offset) == "function" then
    local ok, offset = pcall(utf8.offset, value, length + 1)
    if ok then
      if not offset then
        return value, ""
      end
      return value:sub(1, offset - 1), value:sub(offset)
    end
  end
  return value:sub(1, length), value:sub(length + 1)
end

local function package_menu_line(label, value)
  local line = label and label .. ": " .. tostring(value) or tostring(value)
  if utf8_length(line) <= PACKAGE_MENU_COLUMNS then
    return line
  end
  local prefix = utf8_prefix(line, PACKAGE_MENU_COLUMNS - 1)
  return prefix .. "…"
end

local function add_package_menu_info(menu, id, label, value)
  menu:menuItem {
    id = id,
    text = package_menu_line(label, value),
    enabled = false,
  }
end

local function repository_url(value)
  if type(value) == "table" then
    value = value.url
  end
  if type(value) ~= "string" then
    return nil
  end
  value = value:match("^%s*(.-)%s*$")
  if value == ""
    or value:find("[%c%s]")
    or not value:match("^https://[^/]+")
  then
    return nil
  end
  return value
end

local function package_repository_url(package)
  if type(package) ~= "table" then
    return nil
  end
  local direct = repository_url(package.repository)
  if direct then
    return direct
  end
  if type(package.source) == "table" then
    return repository_url(package.source.repository)
  end
  return nil
end

local function add_repository_menu_action(ui, menu, id, value)
  local url = repository_url(value)
  if not url then
    return false
  end
  menu:menuItem {
    id = id,
    text = "Visit Repository",
    onclick = function()
      ui.app.command.Launch {
        path = url,
      }
    end,
  }
  return true
end

local function wrapped_text_lines(value, columns)
  columns = math.max(1, tonumber(columns) or CONFIRM_LINE_COLUMNS)
  local normalized = tostring(value or "")
    :gsub("\r\n", "\n")
    :gsub("\r", "\n")
  local lines = {}

  local function append_paragraph(paragraph)
    if paragraph:match("^%s*$") then
      lines[#lines + 1] = ""
      return
    end

    local current = ""
    for original_word in paragraph:gmatch("%S+") do
      local word = original_word
      while utf8_length(word) > columns do
        if current ~= "" then
          lines[#lines + 1] = current
          current = ""
        end
        local prefix
        prefix, word = utf8_prefix(word, columns)
        lines[#lines + 1] = prefix
      end

      if current == "" then
        current = word
      elseif utf8_length(current) + 1 + utf8_length(word) <= columns then
        current = current .. " " .. word
      else
        lines[#lines + 1] = current
        current = word
      end
    end
    if current ~= "" then
      lines[#lines + 1] = current
    end
  end

  for paragraph in (normalized .. "\n"):gmatch("(.-)\n") do
    append_paragraph(paragraph)
  end
  if #lines == 0 then
    lines[1] = ""
  end
  return lines
end

local function add_wrapped_labels(dialog, id_prefix, value, columns)
  local lines = wrapped_text_lines(value, columns)
  for index, line in ipairs(lines) do
    dialog:label {
      id = id_prefix .. tostring(index),
      text = line == "" and " " or line,
      hexpand = true,
    }
  end
  return lines
end

local function confirmation_show_bounds(dialog, app_instance, rectangle_constructor)
  local current_width = dialog_dimension(dialog, "sizeHint", "width")
    or dialog_dimension(dialog, "bounds", "width")
  local current_height = dialog_dimension(dialog, "sizeHint", "height")
    or dialog_dimension(dialog, "bounds", "height")
  if not current_width or not current_height then
    return nil
  end

  local window = app_instance and app_instance.window
  local window_width = object_dimension(window, "width")
  local window_height = object_dimension(window, "height")
  local scale = ui_scale(app_instance)
  local margin = MANAGER_WINDOW_MARGIN * scale
  local target_width = math.max(current_width, DEFAULT_CONFIRM_WIDTH * scale)
  local target_height = current_height
  if window_width then
    target_width = math.min(target_width, math.max(1, window_width - margin * 2))
  end
  if window_height then
    target_height = math.min(target_height, math.max(1, window_height - margin * 2))
  end

  local target_x = window_width and math.floor((window_width - target_width) / 2) or 0
  local target_y = window_height and math.floor((window_height - target_height) / 2) or 0
  target_x = math.max(window_width and margin or 0, target_x)
  target_y = math.max(window_height and margin or 0, target_y)

  if type(rectangle_constructor) == "function" then
    return rectangle_constructor(target_x, target_y, target_width, target_height)
  end
  return {
    x = target_x,
    y = target_y,
    width = target_width,
    height = target_height,
  }
end

local function manager_show_bounds(dialog, app_instance, rectangle_constructor)
  local current_width = dialog_dimension(dialog, "bounds", "width")
    or dialog_dimension(dialog, "sizeHint", "width")
  local current_height = dialog_dimension(dialog, "bounds", "height")
    or dialog_dimension(dialog, "sizeHint", "height")
  if not current_width or not current_height then
    return nil
  end

  local window = app_instance and app_instance.window
  local window_width = object_dimension(window, "width")
  local scale = ui_scale(app_instance)
  local margin = MANAGER_WINDOW_MARGIN * scale
  local target_width = math.max(current_width, DEFAULT_MANAGER_WIDTH * scale)
  if window_width then
    target_width = math.min(
      target_width,
      math.max(1, window_width - margin * 2)
    )
  end

  local ok, current = pcall(function()
    return dialog.bounds
  end)
  current = ok and current or nil
  local current_x = tonumber(current and current.x)
  local current_y = tonumber(current and current.y) or 0
  local target_x = current_x
  if target_x then
    target_x = target_x - math.floor((target_width - current_width) / 2)
  end
  if window_width then
    if not target_x or target_x <= 0 then
      target_x = math.floor((window_width - target_width) / 2)
    end
    local maximum_x = math.max(
      margin,
      window_width - target_width - margin
    )
    target_x = math.max(margin, math.min(target_x, maximum_x))
  else
    target_x = math.max(0, target_x or 0)
  end

  local bounds = {
    x = target_x,
    y = current_y,
    width = target_width,
    height = current_height,
  }
  if type(rectangle_constructor) == "function" then
    return rectangle_constructor(bounds.x, bounds.y, bounds.width, bounds.height)
  end
  return bounds
end

local function option_labels(options)
  local labels = {}
  for index, option in ipairs(options) do
    labels[index] = option.label
  end
  return labels
end

local function option_label(options, value)
  for _, option in ipairs(options) do
    if option.value == value then
      return option.label
    end
  end
  return options[1].label
end

local function option_value(options, label)
  for _, option in ipairs(options) do
    if option.label == label then
      return option.value
    end
  end
  return nil
end

local function view_value(model, kind, field)
  local prefix = kind == "browse" and "browse" or "installed"
  local suffix = field == "filter" and "Filter" or "Sort"
  return model[prefix .. suffix]
end

local function add_view_controls(ui, dialog, kind, supports_same_row)
  local options = VIEW_OPTIONS[kind]
  local active = ui:_is_active_view(kind)

  local function add_combobox(field, id, visible, expand)
    local choices = options[field]
    dialog:combobox {
      id = id,
      options = option_labels(choices),
      option = option_label(choices, view_value(ui.model, kind, field)),
      visible = visible,
      hexpand = expand == true,
      onchange = function()
        local selected = option_value(choices, dialog.data[id])
        local changed
        if field == "filter" then
          changed = ui.model:set_filter(kind, selected)
        else
          changed = ui.model:set_sort(kind, selected)
        end
        if changed then
          ui:refresh()
        end
      end,
    }
  end

  if supports_same_row then
    dialog:samerow()
    add_combobox("filter", kind .. "_filter", active, false)
    dialog:samerow()
    add_combobox("sort", kind .. "_sort", active, false)
  else
    add_combobox("filter", kind .. "_filter", active, true)
    add_combobox("sort", kind .. "_sort", active, true)
  end
  dialog:newrow()

  if supports_same_row then
    add_combobox("filter", kind .. "_stacked_filter", false, true)
    dialog:newrow()
    add_combobox("sort", kind .. "_stacked_sort", false, true)
    dialog:newrow()
  end
end

local function add_search_control(ui, dialog, kind, supports_same_row)
  local id = kind .. "_search"
  local active = ui:_is_active_view(kind)
  if supports_same_row then
    dialog:label {
      id = kind .. "_search_label",
      text = "Search:",
      hexpand = false,
      visible = active,
    }
    dialog:samerow()
  end
  local search_text
  if kind == "browse" then
    search_text = ui.model.browseSearch
  elseif kind == "installed" then
    search_text = ui.model.installedSearch
  else
    search_text = ui.model.githubSearch
  end
  dialog:entry {
    id = id,
    label = not supports_same_row and "Search:" or nil,
    text = search_text,
    hexpand = true,
    visible = active,
    onchange = function()
      if kind == "github" then
        ui.controller:search_github_repositories(dialog.data[id])
      else
        ui.model:set_search(kind, dialog.data[id])
        ui:refresh()
      end
    end,
  }
end

local function add_pager(ui, dialog, kind, supports_same_row)
  local active = ui:_is_active_view(kind)
  local function stay_on_row()
    if supports_same_row then
      dialog:samerow()
    end
  end

  if supports_same_row then
    dialog:label {
      id = kind .. "_pager_left_spacer",
      text = "",
      hexpand = true,
      visible = active,
    }
    stay_on_row()
  end
  dialog:button {
    id = kind .. "_previous",
    text = "←",
    focus = false,
    hexpand = false,
    visible = active,
    onclick = function()
      if kind == "github" then
        ui.controller:move_github_page(-1)
      else
        ui.model:move_page(kind, -1)
        ui:refresh()
      end
    end,
  }
  stay_on_row()
  if supports_same_row then
    dialog:label {
      id = kind .. "_page",
      text = "1 / 1",
      hexpand = false,
      visible = active,
    }
  else
    dialog:button {
      id = kind .. "_page",
      text = "1 / 1",
      enabled = false,
      focus = false,
      hexpand = false,
      visible = active,
    }
  end
  stay_on_row()
  dialog:button {
    id = kind .. "_next",
    text = "→",
    focus = false,
    hexpand = false,
    visible = active,
    onclick = function()
      if kind == "github" then
        ui.controller:move_github_page(1)
      else
        ui.model:move_page(kind, 1)
        ui:refresh()
      end
    end,
  }
  if supports_same_row then
    stay_on_row()
    dialog:label {
      id = kind .. "_pager_right_spacer",
      text = "",
      hexpand = true,
      visible = active,
    }
  end
  dialog:newrow()
end

local function update_pager(
  dialog,
  kind,
  page,
  pages,
  busy,
  supports_same_row,
  update_visibility,
  can_move_next
)
  local active = update_visibility == true
  local visible = true
  local page_update = {
    id = kind .. "_page",
    text = tostring(page) .. " / " .. tostring(pages),
    visible = active and visible,
  }
  local previous_update = {
    id = kind .. "_previous",
    enabled = visible and page > 1 and not busy,
    visible = active and visible,
  }
  local next_update = {
    id = kind .. "_next",
    enabled = visible
      and page < pages
      and can_move_next ~= false
      and not busy,
    visible = active and visible,
  }
  dialog:modify(page_update)
  dialog:modify(previous_update)
  dialog:modify(next_update)
  if supports_same_row then
    local left_spacer_update = {
      id = kind .. "_pager_left_spacer",
      visible = active and visible,
    }
    local right_spacer_update = {
      id = kind .. "_pager_right_spacer",
      visible = active and visible,
    }
    dialog:modify(left_spacer_update)
    dialog:modify(right_spacer_update)
  end
end

local Progress = {}
Progress.__index = Progress

function Progress:update(event)
  if self.cancelled or not self.dialog then
    return
  end
  local message = event and event.message or "Working…"
  if event and event.current and event.total then
    message = message .. " (" .. tostring(event.current) .. "/" .. tostring(event.total) .. ")"
  end
  pcall(function()
    self.dialog:modify {
      id = "progress_message",
      text = message,
    }
  end)
end

function Progress:attach(ticket)
  self.ticket = ticket
  if self.cancelled and ticket then
    ticket.cancel()
  end
end

function Progress:close()
  local dialog = self.dialog
  self.dialog = nil
  if dialog then
    self.closing = true
    pcall(function()
      dialog:close()
    end)
  end
end

function Progress:is_cancelled()
  return self.cancelled == true
end

function Ui.new(environment, model)
  return setmetatable({
    environment = environment,
    app = environment.app,
    model = model,
    controller = nil,
    dialog = nil,
    browseRows = {},
    installedRows = {},
    githubRows = {},
    githubAvailable = false,
    githubAuthenticated = false,
    githubDiagnosticsTicket = nil,
    suppressClose = false,
    supportsSameRow = false,
    responsiveReady = false,
    rowsStacked = false,
    wideRowMinWidth = nil,
    layoutTimer = nil,
    pendingLayoutWidth = nil,
    activeView = "browse",
    screen = "manager",
  }, Ui)
end

function Ui:_parent()
  return self.dialog
end

function Ui:_cancel_github_diagnostics()
  local ticket = self.githubDiagnosticsTicket
  self.githubDiagnosticsTicket = nil
  if ticket then
    ticket.cancel()
  end
end

function Ui:_check_github_tools()
  self:_cancel_github_diagnostics()
  local dialog = self.dialog
  if not dialog
    or not self.controller
    or type(self.controller.request_diagnostics) ~= "function"
  then
    return false
  end

  local completed = false
  local ticket = self.controller:request_diagnostics(function(result, error_value)
    completed = true
    if self.dialog ~= dialog then
      return
    end
    self.githubDiagnosticsTicket = nil
    local tools = type(result) == "table" and result.tools or nil
    local git = type(tools) == "table" and tools.git or nil
    local gh = type(tools) == "table" and tools.gh or nil
    self.githubAvailable = error_value == nil
      and type(git) == "table"
      and git.installed == true
      and type(gh) == "table"
      and gh.installed == true
    self.githubAuthenticated = self.githubAvailable and gh.authenticated == true
    self:refresh()
    if self.githubAuthenticated
      and self.activeView == "github"
      and not self.model.githubLoaded
      and not self.model.githubLoading
      and type(self.controller.search_github_repositories) == "function"
    then
      self.controller:search_github_repositories(self.model.githubSearch)
    end
  end)
  if not completed then
    self.githubDiagnosticsTicket = ticket
  end
  return ticket ~= nil
end

function Ui:_stop_layout_timer()
  local timer = self.layoutTimer
  self.layoutTimer = nil
  self.pendingLayoutWidth = nil
  if timer then
    pcall(function()
      timer:stop()
    end)
  end
end

function Ui:_fallback_inline_width()
  return FALLBACK_INLINE_WIDTH * ui_scale(self.app)
end

function Ui:_capture_wide_row_width()
  if not self.supportsSameRow or self.rowsStacked or not self.dialog then
    return
  end
  self.wideRowMinWidth = self:_fallback_inline_width()
end

function Ui:_row_package(kind, index)
  if kind == "browse" then
    return self.browseRows[index]
  elseif kind == "installed" then
    return self.installedRows[index]
  end
  return self.githubRows[index]
end

function Ui:_row_text(kind, package)
  if not package then
    return ""
  end
  if self.supportsSameRow and self.rowsStacked then
    if kind == "browse" then
      return narrow_catalog_row(package)
    elseif kind == "installed" then
      return narrow_installed_row(package)
    end
    return github_repository_row(package)
  end
  if kind == "browse" then
    return catalog_row(package)
  elseif kind == "installed" then
    return installed_row(package)
  end
  return github_repository_row(package)
end

function Ui:_is_active_view(kind)
  return self.screen == "manager" and self.activeView == kind
end

function Ui:_update_screen_controls()
  local dialog = self.dialog
  if not dialog then
    return
  end
  local manager_visible = self.screen == "manager"
  for _, id in ipairs({
    "manager_tabs",
    "manager_footer_separator",
    "manager_status",
    "install",
    "refresh",
    "preferences",
    "help",
  }) do
    dialog:modify {
      id = id,
      visible = manager_visible,
    }
  end
  if self.supportsSameRow then
    dialog:modify {
      id = "manager_footer_lead",
      visible = manager_visible,
    }
  end
  for _, id in ipairs({
    "preferences_back",
    "preferences_title",
    "startup_checks",
    "preferences_description",
    "preferences_local_data",
    "preferences_data_path",
    "preferences_clear_cache",
  }) do
    dialog:modify {
      id = id,
      visible = not manager_visible,
    }
  end
  dialog:modify {
    id = "preferences_clear_cache",
    enabled = not self.model.busy,
  }
end

function Ui:_show_manager_screen()
  if self.screen == "manager" then
    return
  end
  self.screen = "manager"
  self:refresh()
end

function Ui:_activate_tab(tab)
  local kind
  if tab == "browse_tab" then
    kind = "browse"
  elseif tab == "installed_tab" then
    kind = "installed"
  elseif tab == "github_tab" and self.githubAvailable then
    kind = "github"
  end
  if not kind or self.activeView == kind then
    return
  end

  local previous_autofit
  local autofit_suspended = false
  if self.responsiveReady and self.dialog then
    autofit_suspended = pcall(function()
      previous_autofit = self.dialog.autofit
      self.dialog.autofit = 0
    end)
  end
  self.activeView = kind
  if kind == "github"
    and self.githubAuthenticated
    and not self.model.githubLoaded
    and not self.model.githubLoading
    and self.controller
    and type(self.controller.search_github_repositories) == "function"
  then
    self.controller:search_github_repositories(self.model.githubSearch)
  end
  if self.responsiveReady then
    self:refresh()
  end
  if autofit_suspended and self.dialog then
    pcall(function()
      self.dialog.autofit = previous_autofit
      self.dialog:modify {
        id = "manager_tabs",
        visible = self.screen == "manager",
      }
    end)
  end
end

function Ui:_update_search_control(kind)
  local dialog = self.dialog
  if not dialog then
    return
  end
  local active = self:_is_active_view(kind)
  if self.supportsSameRow then
    dialog:modify {
      id = kind .. "_search_label",
      visible = active,
    }
  end
  dialog:modify {
    id = kind .. "_search",
    visible = active,
    enabled = kind ~= "github" or self.githubAuthenticated,
  }
end

function Ui:_update_row_controls(kind, index)
  local dialog = self.dialog
  if not dialog then
    return
  end
  local package = self:_row_package(kind, index)
  local present = package ~= nil
  local stacked = self.supportsSameRow and self.rowsStacked
  local active = self:_is_active_view(kind)
  local enabled = not self.model.busy
    and (kind ~= "github" or (self.githubAuthenticated and not self.model.githubLoading))
  if not self.supportsSameRow then
    local details_update = {
      id = kind .. "_details_" .. tostring(index),
      label = self:_row_text(kind, package),
      enabled = enabled,
      visible = active and present,
    }
    dialog:modify(details_update)
    return
  end
  local row_update = {
    id = kind .. "_row_" .. tostring(index),
    text = self:_row_text(kind, package),
    visible = active and present,
  }
  local details_update = {
    id = kind .. "_details_" .. tostring(index),
    enabled = enabled,
    visible = active and present and not stacked,
  }
  dialog:modify(row_update)
  dialog:modify(details_update)
  if self.supportsSameRow then
    local stacked_details_update = {
      id = kind .. "_stacked_details_" .. tostring(index),
      enabled = enabled,
      visible = active and present and stacked,
    }
    dialog:modify(stacked_details_update)
  end
end

function Ui:_update_view_controls(kind)
  local dialog = self.dialog
  if not dialog then
    return
  end
  local options = VIEW_OPTIONS[kind]
  local stacked = self.supportsSameRow and self.rowsStacked
  local active = self:_is_active_view(kind)
  for _, field in ipairs({ "filter", "sort" }) do
    local selected = option_label(options[field], view_value(self.model, kind, field))
    local inline_update = {
      id = kind .. "_" .. field,
      option = selected,
      visible = active and not stacked,
    }
    dialog:modify(inline_update)
    if self.supportsSameRow then
      local stacked_update = {
        id = kind .. "_stacked_" .. field,
        option = selected,
        visible = active and stacked,
      }
      dialog:modify(stacked_update)
    end
  end
end

function Ui:_apply_row_layout(stacked, current_width)
  if not self.responsiveReady or not self.supportsSameRow or not self.dialog then
    return
  end
  stacked = stacked == true
  if self.rowsStacked == stacked then
    return
  end
  self.rowsStacked = stacked
  for _, kind in ipairs({ "browse", "installed", "github" }) do
    if VIEW_OPTIONS[kind] then
      self:_update_view_controls(kind)
    end
    for index = 1, self.model.pageSize do
      self:_update_row_controls(kind, index)
    end
  end
  if not stacked then
    self:_capture_wide_row_width()
    if current_width and current_width < self.wideRowMinWidth then
      self:_apply_row_layout(true, current_width)
    end
  end
end

function Ui:_apply_layout_width(width)
  local minimum = self.wideRowMinWidth or self:_fallback_inline_width()
  self:_apply_row_layout(width < minimum, width)
end

function Ui:_schedule_row_layout(dialog)
  if not self.responsiveReady or not self.supportsSameRow or self.dialog ~= dialog then
    return
  end
  local width = dialog_dimension(dialog, "bounds", "width")
  if not width then
    return
  end

  local Timer = self.environment.Timer
  if not Timer then
    self:_apply_layout_width(width)
    return
  end

  local old_timer = self.layoutTimer
  self.layoutTimer = nil
  if old_timer then
    pcall(function()
      old_timer:stop()
    end)
  end
  self.pendingLayoutWidth = width

  local timer
  timer = Timer {
    interval = RESIZE_DEBOUNCE_SECONDS,
    ontick = function()
      if self.layoutTimer ~= timer then
        return
      end
      timer:stop()
      self.layoutTimer = nil
      local pending_width = self.pendingLayoutWidth
      self.pendingLayoutWidth = nil
      if self.dialog == dialog then
        local final_width = dialog_dimension(dialog, "bounds", "width") or pending_width
        if final_width then
          self:_apply_layout_width(final_width)
        end
      end
    end,
  }
  self.layoutTimer = timer
  timer:start()
end

function Ui:show_compatibility(message)
  self.app.alert {
    title = "Aseprite Extension Manager",
    text = message,
    buttons = "OK",
  }
end

function Ui:show_onboarding()
  local dialog = self.environment.Dialog {
    title = "Welcome to Aseprite Extension Manager",
    resizeable = true,
  }
  dialog:separator { text = "Private Alpha" }
  dialog:label {
    text = "Install public or private GitHub extensions and link local development folders.",
  }
  dialog:separator { text = "Permissions" }
  dialog:label {
    text = "Aseprite will ask for permission to run the bundled helper.",
  }
  dialog:label {
    text = "It will also ask to connect to that helper on localhost.",
  }
  dialog:label {
    text = "No broad access is requested. There is no usage reporting.",
  }
  dialog:separator {}
  dialog:button {
    id = "continue_onboarding",
    text = "Continue",
    focus = true,
  }
  dialog:button {
    id = "cancel_onboarding",
    text = "Not Now",
  }
  show_responsive(dialog)
  return dialog.data.continue_onboarding == true
end

function Ui:_build()
  local dialog
  dialog = self.environment.Dialog {
    title = "Aseprite Extension Manager",
    resizeable = true,
    onresize = function()
      self:_schedule_row_layout(dialog)
    end,
    onclose = function()
      self:_cancel_github_diagnostics()
      self:_stop_layout_timer()
      self.responsiveReady = false
      if self.dialog == dialog then
        self.dialog = nil
      end
      if not self.suppressClose and self.controller then
        self.controller:on_dialog_closed()
      end
    end,
  }
  self.dialog = dialog
  self.supportsSameRow = type(dialog.samerow) == "function"
  self.rowsStacked = false
  self.wideRowMinWidth = nil
  self.activeView = "browse"
  self.screen = "manager"

  dialog:tab {
    id = "browse_tab",
    text = "Browse",
  }
  add_search_control(self, dialog, "browse", self.supportsSameRow)
  add_view_controls(self, dialog, "browse", self.supportsSameRow)
  dialog:label {
    id = "browse_empty",
    text = "",
    hexpand = true,
  }
  if self.supportsSameRow then
    dialog:newrow()
  end
  for index = 1, self.model.pageSize do
    local row_index = index
    if self.supportsSameRow then
      dialog:label {
        id = "browse_row_lead_" .. tostring(index),
        text = "",
        visible = false,
        hexpand = false,
      }
      dialog:samerow()
    end
    dialog:button {
      id = "browse_details_" .. tostring(index),
      label = not self.supportsSameRow and "" or nil,
      text = "Edit ▾",
      visible = false,
      hexpand = false,
      onclick = function()
        local package = self.browseRows[row_index]
        if package then
          self:show_package_menu(package, "browse")
        end
      end,
    }
    if self.supportsSameRow then
      dialog:samerow()
      dialog:label {
        id = "browse_row_" .. tostring(index),
        text = "",
        visible = false,
        hexpand = false,
      }
    end
    dialog:newrow()
    if self.supportsSameRow then
      dialog:button {
        id = "browse_stacked_details_" .. tostring(index),
        text = "Edit ▾",
        visible = false,
        hexpand = false,
        onclick = function()
          local package = self.browseRows[row_index]
          if package then
            self:show_package_menu(package, "browse")
          end
        end,
      }
      dialog:newrow()
    end
  end
  add_pager(self, dialog, "browse", self.supportsSameRow)

  dialog:tab {
    id = "github_tab",
    text = "GitHub",
    visible = false,
    enabled = false,
  }
  add_search_control(self, dialog, "github", self.supportsSameRow)
  dialog:label {
    id = "github_empty",
    text = "",
    visible = false,
    hexpand = true,
  }
  if self.supportsSameRow then
    dialog:newrow()
  end
  for index = 1, self.model.pageSize do
    local row_index = index
    if self.supportsSameRow then
      dialog:label {
        id = "github_row_lead_" .. tostring(index),
        text = "",
        visible = false,
        hexpand = false,
      }
      dialog:samerow()
    end
    dialog:button {
      id = "github_details_" .. tostring(index),
      label = not self.supportsSameRow and "" or nil,
      text = "Install ▾",
      visible = false,
      focus = false,
      hexpand = false,
      onclick = function()
        local repository = self.githubRows[row_index]
        if repository then
          self:show_github_repository_menu(repository)
        end
      end,
    }
    if self.supportsSameRow then
      dialog:samerow()
      dialog:label {
        id = "github_row_" .. tostring(index),
        text = "",
        visible = false,
        hexpand = false,
      }
    end
    dialog:newrow()
    if self.supportsSameRow then
      dialog:button {
        id = "github_stacked_details_" .. tostring(index),
        text = "Install ▾",
        visible = false,
        focus = false,
        hexpand = false,
        onclick = function()
          local repository = self.githubRows[row_index]
          if repository then
            self:show_github_repository_menu(repository)
          end
        end,
      }
      dialog:newrow()
    end
  end
  add_pager(self, dialog, "github", self.supportsSameRow)

  dialog:tab {
    id = "installed_tab",
    text = "Installed",
  }
  add_search_control(self, dialog, "installed", self.supportsSameRow)
  add_view_controls(self, dialog, "installed", self.supportsSameRow)
  dialog:label {
    id = "installed_empty",
    text = "No user extensions were found.",
    hexpand = true,
  }
  if self.supportsSameRow then
    dialog:newrow()
  end
  for index = 1, self.model.pageSize do
    local row_index = index
    if self.supportsSameRow then
      dialog:label {
        id = "installed_row_lead_" .. tostring(index),
        text = "",
        visible = false,
        hexpand = false,
      }
      dialog:samerow()
    end
    dialog:button {
      id = "installed_details_" .. tostring(index),
      label = not self.supportsSameRow and "" or nil,
      text = "Edit ▾",
      visible = false,
      hexpand = false,
      onclick = function()
        local package = self.installedRows[row_index]
        if package then
          self:show_package_menu(package, "installed")
        end
      end,
    }
    if self.supportsSameRow then
      dialog:samerow()
      dialog:label {
        id = "installed_row_" .. tostring(index),
        text = "",
        visible = false,
        hexpand = false,
      }
    end
    dialog:newrow()
    if self.supportsSameRow then
      dialog:button {
        id = "installed_stacked_details_" .. tostring(index),
        text = "Edit ▾",
        visible = false,
        hexpand = false,
        onclick = function()
          local package = self.installedRows[row_index]
          if package then
            self:show_package_menu(package, "installed")
          end
        end,
      }
      dialog:newrow()
    end
  end
  add_pager(self, dialog, "installed", self.supportsSameRow)
  dialog:endtabs {
    id = "manager_tabs",
    selected = "browse_tab",
    hexpand = true,
    vexpand = true,
    onchange = function(event)
      self:_activate_tab(event and event.tab)
    end,
  }

  local preferences = self.environment.plugin.preferences
  if self.supportsSameRow then
    dialog:label {
      id = "preferences_header_lead",
      text = "",
      visible = false,
      hexpand = false,
    }
    dialog:samerow()
  end
  dialog:button {
    id = "preferences_back",
    text = "← Back",
    visible = false,
    focus = false,
    hexpand = false,
    onclick = function()
      self:_show_manager_screen()
    end,
  }
  if self.supportsSameRow then
    dialog:samerow()
  end
  dialog:label {
    id = "preferences_title",
    text = "Preferences",
    visible = false,
    hexpand = false,
  }
  dialog:newrow()
  dialog:check {
    id = "startup_checks",
    text = "Check for updates at startup",
    selected = preferences.startupChecks ~= false,
    visible = false,
    onclick = function()
      preferences.startupChecks = dialog.data.startup_checks == true
    end,
  }
  dialog:label {
    id = "preferences_description",
    text = "Checks linked sources and manager releases at most once every 24 hours.",
    visible = false,
    hexpand = true,
  }
  dialog:separator {
    id = "preferences_local_data",
    text = "Local Data",
    visible = false,
  }
  dialog:label {
    id = "preferences_data_path",
    text = self.app.fs.joinPath(self.app.fs.userConfigPath, "extension-manager"),
    visible = false,
    hexpand = true,
  }
  dialog:button {
    id = "preferences_clear_cache",
    text = "Clear Cache…",
    visible = false,
    focus = false,
    hexpand = false,
    onclick = function()
      if self:confirm(
        "Clear Download Cache",
        "Remove cached package artifacts that are not needed for the current restore point?",
        "Clear Cache"
      ) then
        self.controller:clear_cache()
      end
    end,
  }

  dialog:separator {
    id = "manager_footer_separator",
  }
  dialog:label {
    id = "manager_status",
    text = self.model.status,
    hexpand = true,
  }
  dialog:newrow()
  local compact_footer = self.supportsSameRow
  if compact_footer then
    dialog:label {
      id = "manager_footer_lead",
      text = "",
      hexpand = false,
    }
    dialog:samerow { always = true }
  end
  dialog:button {
    id = "install",
    text = "Install… +",
    focus = false,
    onclick = function()
      local menu = self.environment.Dialog {
        parent = dialog,
      }
      menu:menuItem {
        id = "install_github",
        text = "GitHub…",
        onclick = function()
          self.controller:install_from_github()
        end,
      }
      menu:menuItem {
        id = "sync_local",
        text = "Local Folder…",
        onclick = function()
          self.controller:sync_local_folder()
        end,
      }
      menu:showMenu()
    end,
  }
  if not compact_footer then
    dialog:newrow()
  end
  dialog:button {
    id = "manager_update",
    text = "↑",
    visible = false,
    focus = false,
    hexpand = false,
    onclick = function()
      local manager = self.model.managerPackage
      if self_update_available(manager) then
        self.controller:update_package(manager)
      end
    end,
  }
  dialog:button {
    id = "refresh",
    text = "↻",
    focus = false,
    hexpand = false,
    onclick = function()
      self:_check_github_tools()
      self.controller:refresh(true)
    end,
  }
  dialog:button {
    id = "preferences",
    text = "⚙",
    focus = false,
    hexpand = false,
    onclick = function()
      self:show_preferences()
    end,
  }
  dialog:button {
    id = "help",
    text = "⍰",
    focus = false,
    hexpand = false,
    onclick = function()
      self:show_help()
    end,
  }
  self.responsiveReady = true
end

function Ui:open(controller)
  self.controller = controller
  if not self.dialog then
    self:_build()
  end
  self:refresh()
  show_responsive(self.dialog, {
    wait = false,
    autoscrollbars = false,
  })
  local bounds = manager_show_bounds(
    self.dialog,
    self.app,
    self.environment.Rectangle
  )
  if bounds then
    pcall(function()
      self.dialog.bounds = bounds
    end)
  end
  pcall(function()
    self.dialog.autofit = WIDTH_PRESERVING_AUTOFIT
  end)
  self:_schedule_row_layout(self.dialog)
  self:_check_github_tools()
end

function Ui:close()
  self:_cancel_github_diagnostics()
  self:_stop_layout_timer()
  self.responsiveReady = false
  local dialog = self.dialog
  self.dialog = nil
  if dialog then
    self.suppressClose = true
    pcall(function()
      dialog:close()
    end)
    self.suppressClose = false
  end
end

function Ui:set_busy(busy, status)
  self.model.busy = busy == true
  if status then
    self.model.status = status
  end
  self:refresh()
end

function Ui:refresh()
  local dialog = self.dialog
  if not dialog then
    return
  end

  local browse, browse_page, browse_pages, browse_count = self.model:page("browse")
  self.browseRows = browse
  local empty_message
  local browse_has_search = self.model.browseSearch ~= ""
  local browse_has_filter = self.model.browseFilter ~= "all"
  if browse_count == 0 and browse_has_search and browse_has_filter then
    empty_message = "No catalog packages match this search and filter."
  elseif browse_count == 0 and browse_has_search then
    empty_message = "No catalog packages match this search."
  elseif browse_count == 0 and browse_has_filter then
    empty_message = "No catalog packages match this filter."
  elseif browse_count == 0 then
    empty_message = "Catalog is empty. Use Install… to add an extension."
  else
    empty_message = ""
  end
  if self.model.registryExpired then
    local expired_message = "Catalog metadata is expired. Cached entries are view-only."
    empty_message = empty_message == "" and expired_message
      or empty_message .. " " .. expired_message
  end
  dialog:modify {
    id = "browse_empty",
    text = empty_message,
    visible = self:_is_active_view("browse") and empty_message ~= "",
  }
  self:_update_search_control("browse")
  self:_update_view_controls("browse")
  for index = 1, self.model.pageSize do
    self:_update_row_controls("browse", index)
  end
  update_pager(
    dialog,
    "browse",
    browse_page,
    browse_pages,
    self.model.busy,
    self.supportsSameRow,
    self:_is_active_view("browse")
  )

  local installed, installed_page, installed_pages, installed_count = self.model:page("installed")
  self.installedRows = installed
  local installed_empty = installed_count == 0
  local installed_has_search = self.model.installedSearch ~= ""
  local installed_has_filter = self.model.installedFilter ~= "all"
  local installed_empty_message
  if installed_has_search and installed_has_filter then
    installed_empty_message = "No installed extensions match this search and filter."
  elseif installed_has_search then
    installed_empty_message = "No installed extensions match this search."
  elseif installed_has_filter then
    installed_empty_message = "No installed extensions match this filter."
  else
    installed_empty_message = "No user extensions were found."
  end
  dialog:modify {
    id = "installed_empty",
    text = installed_empty_message,
    visible = self:_is_active_view("installed") and installed_empty,
  }
  self:_update_search_control("installed")
  self:_update_view_controls("installed")
  for index = 1, self.model.pageSize do
    self:_update_row_controls("installed", index)
  end
  update_pager(
    dialog,
    "installed",
    installed_page,
    installed_pages,
    self.model.busy,
    self.supportsSameRow,
    self:_is_active_view("installed")
  )

  dialog:modify {
    id = "github_tab",
    visible = self.githubAvailable,
    enabled = self.githubAvailable,
  }
  local github, github_page, github_pages = self.model:page("github")
  self.githubRows = github
  local github_empty_message = ""
  if not self.githubAuthenticated then
    github_empty_message = "Sign in with gh auth login to browse accessible repositories."
  elseif self.model.githubLoading then
    github_empty_message = "Checking repositories for Aseprite extensions..."
  elseif self.model.githubError then
    github_empty_message = Protocol.error_message(self.model.githubError)
  elseif self.model.githubLoaded
    and #github == 0
    and self.model.githubHasNextPage
  then
    github_empty_message = "No Aseprite extensions on this page. Try the next page."
  elseif self.model.githubLoaded and #github == 0 and self.model.githubSearch ~= "" then
    github_empty_message = "No Aseprite extension repositories match this search."
  elseif self.model.githubLoaded and #github == 0 then
    github_empty_message = "No Aseprite extension repositories were found."
  end
  dialog:modify {
    id = "github_empty",
    text = github_empty_message,
    visible = self:_is_active_view("github") and github_empty_message ~= "",
  }
  self:_update_search_control("github")
  for index = 1, self.model.pageSize do
    self:_update_row_controls("github", index)
  end
  update_pager(
    dialog,
    "github",
    github_page,
    github_pages,
    self.model.busy or self.model.githubLoading,
    self.supportsSameRow,
    self:_is_active_view("github") and self.githubAuthenticated,
    self.model.githubHasNextPage
  )

  local manager_update = self_update_available(self.model.managerPackage)
  local status = self.model.status
  if manager_update then
    local version = self.model.managerPackage.update.version
    if version then
      status = status .. " · Manager update v" .. tostring(version) .. " available"
    else
      status = status .. " · Manager update available"
    end
  end
  dialog:modify {
    id = "manager_status",
    text = status,
  }
  dialog:modify {
    id = "manager_update",
    visible = self.screen == "manager" and manager_update,
    enabled = manager_update and not self.model.busy,
  }
  for _, id in ipairs({
    "install",
    "refresh",
    "preferences",
  }) do
    dialog:modify {
      id = id,
      enabled = not self.model.busy,
    }
  end
  self:_update_screen_controls()
  self:_capture_wide_row_width()
  self:_schedule_row_layout(dialog)
end

function Ui:show_error(title, error_value)
  local message = Protocol.error_message(error_value)
  local dialog = self.environment.Dialog {
    title = title or "Aseprite Extension Manager",
    parent = self:_parent(),
    resizeable = true,
  }
  dialog:label {
    text = message,
    hexpand = true,
  }
  if type(error_value) == "table" and error_value.code then
    dialog:label {
      label = "Code:",
      text = tostring(error_value.code),
    }
  end
  local release_url = type(error_value) == "table"
      and type(error_value.details) == "table"
      and error_value.details.releaseUrl
    or nil
  local recovery_artifact = type(error_value) == "table"
      and type(error_value.details) == "table"
      and error_value.details.recoveryArtifact
    or nil
  if type(recovery_artifact) == "string" and recovery_artifact ~= "" then
    dialog:label {
      label = "Recovery package:",
      text = recovery_artifact,
      hexpand = true,
    }
  end
  if error_value
      and (error_value.code == "SELF_UPDATE_MANUAL"
        or error_value.code == "SELF_UPDATE_RESTRICTED"
        or error_value.code == "SELF_UPDATE_RECOVERY_REQUIRED")
      and type(release_url) == "string"
      and release_url:match("^https://github%.com/")
  then
    dialog:button {
      text = "Open Releases",
      onclick = function()
        self.app.command.Launch {
          path = release_url,
        }
      end,
    }
  end
  dialog:button {
    text = "OK",
    focus = true,
  }
  show_responsive(dialog)
end

function Ui:confirm(title, message, confirm_text, cancel_text)
  local dialog = self.environment.Dialog {
    title = title,
    parent = self:_parent(),
    resizeable = true,
  }
  add_wrapped_labels(dialog, "confirm_message_", message, CONFIRM_LINE_COLUMNS)
  dialog:button {
    id = "confirm_action",
    text = confirm_text or "Continue",
    focus = true,
  }
  dialog:button {
    id = "cancel_action",
    text = cancel_text or "Cancel",
  }
  local show_options = {}
  local bounds = confirmation_show_bounds(
    dialog,
    self.app,
    self.environment.Rectangle
  )
  if bounds then
    show_options.bounds = bounds
  end
  show_responsive(dialog, show_options)
  return dialog.data.confirm_action == true
end

function Ui:show_progress(title, initial_message, on_cancel)
  local progress = setmetatable({
    cancelled = false,
    closing = false,
    cancelNotified = false,
    ticket = nil,
  }, Progress)
  local dialog = self.environment.Dialog {
    title = title,
    parent = self:_parent(),
    resizeable = true,
    onclose = function()
      if not progress.closing then
        progress.cancelled = true
        if progress.ticket then
          progress.ticket.cancel()
        end
        if on_cancel and not progress.cancelNotified then
          progress.cancelNotified = true
          on_cancel()
        end
      end
      progress.dialog = nil
    end,
  }
  progress.dialog = dialog
  dialog:label {
    id = "progress_message",
    text = initial_message or "Working…",
    hexpand = true,
  }
  dialog:button {
    text = "Cancel",
    onclick = function()
      dialog:close()
    end,
  }
  show_responsive(dialog, {
    wait = false,
  })
  pcall(function()
    dialog.autofit = WIDTH_PRESERVING_AUTOFIT
  end)
  return progress
end

function Ui:prompt_github_url()
  local dialog = self.environment.Dialog {
    title = "Install from GitHub",
    parent = self:_parent(),
    resizeable = true,
  }
  dialog:label {
    id = "github_prompt_source",
    text = "Enter a public or private GitHub repository URL or",
  }
  dialog:label {
    text = "an .aseprite-extension release URL.",
  }
  dialog:entry {
    id = "github_url",
    label = "URL:",
    text = "",
    focus = true,
    hexpand = true,
  }
  dialog:button {
    id = "install_github_url",
    text = "Continue",
    focus = true,
  }
  dialog:button {
    id = "cancel_github_url",
    text = "Cancel",
  }
  show_responsive(dialog)
  if dialog.data.install_github_url then
    return dialog.data.github_url
  end
  return nil
end

function Ui:choose_github_asset(choices)
  if type(choices) ~= "table" or #choices == 0 then
    return nil
  end
  local labels = {}
  local by_label = {}
  for index, choice in ipairs(choices) do
    local label = text(choice.name, text(choice.label, "Asset " .. tostring(index)))
    if by_label[label] then
      label = label .. " (" .. tostring(index) .. ")"
    end
    labels[#labels + 1] = label
    by_label[label] = tostring(
      choice.id or choice.assetId or choice.name or index
    )
  end

  local dialog = self.environment.Dialog {
    title = "Choose Release Asset",
    parent = self:_parent(),
    resizeable = true,
  }
  dialog:label {
    text = "More than one compatible extension asset was found.",
  }
  dialog:combobox {
    id = "github_asset",
    label = "Asset:",
    options = labels,
    option = labels[1],
    hexpand = true,
  }
  dialog:button {
    id = "choose_github_asset",
    text = "Continue",
    focus = true,
  }
  dialog:button {
    id = "cancel_github_asset",
    text = "Cancel",
  }
  show_responsive(dialog)
  if not dialog.data.choose_github_asset then
    return nil
  end
  return by_label[dialog.data.github_asset]
end

function Ui:prompt_package_json()
  local dialog = self.environment.Dialog {
    title = "Link Local Folder",
    parent = self:_parent(),
    resizeable = true,
  }
  dialog:label {
    text = "Select the local extension folder's package.json. The manager remembers this folder",
  }
  dialog:label {
    text = "and detects file changes during update checks. Aseprite installs a safe snapshot.",
  }
  dialog:file {
    id = "package_json",
    title = "Select package.json",
    filename = "",
    open = true,
    save = false,
    entry = true,
    filetypes = {
      "json",
    },
  }
  dialog:button {
    id = "sync_package_json",
    text = "Link and Install",
    focus = true,
  }
  dialog:button {
    id = "cancel_package_json",
    text = "Cancel",
  }
  show_responsive(dialog)
  if dialog.data.sync_package_json then
    return dialog.data.package_json
  end
  return nil
end

function Ui:show_help()
  local help_open = true
  local diagnostics_ticket
  local dialog = self.environment.Dialog {
    title = "About Aseprite Extension Manager",
    parent = self:_parent(),
    resizeable = true,
    onclose = function()
      help_open = false
      if diagnostics_ticket then
        diagnostics_ticket.cancel()
        diagnostics_ticket = nil
      end
    end,
  }
  dialog:label {
    id = "help_version",
    label = "Version:",
    text = manager_version(self.environment.plugin),
  }
  dialog:label {
    id = "help_creator",
    label = "Created by:",
    text = "Martin Calander · Soupmasters",
  }
  dialog:label {
    id = "help_license",
    label = "License:",
    text = "MIT License",
  }
  local repository_link_pressed = false
  dialog:canvas {
    id = "help_repository",
    label = "Repository:",
    width = REPOSITORY_LINK_WIDTH,
    height = REPOSITORY_LINK_HEIGHT,
    hexpand = false,
    vexpand = false,
    onpaint = function(event)
      paint_link(event, REPOSITORY_LINK_TEXT, repository_link_pressed)
    end,
    onmousedown = function(event)
      repository_link_pressed = event.button == LEFT_MOUSE_BUTTON
        and inside_repository_link(event)
      dialog:repaint()
    end,
    onmouseup = function(event)
      local should_open = repository_link_pressed
        and event.button == LEFT_MOUSE_BUTTON
        and inside_repository_link(event)
      repository_link_pressed = false
      dialog:repaint()
      if should_open then
        self.app.command.Launch {
          path = REPOSITORY_URL,
        }
      end
    end,
  }
  if POINTER_CURSOR then
    pcall(function()
      dialog:modify {
        id = "help_repository",
        mouseCursor = POINTER_CURSOR,
      }
    end)
  end
  dialog:separator { text = "Command Line Tools" }
  dialog:label {
    id = "help_git",
    label = "Git:",
    text = "? Checking…",
  }
  dialog:label {
    id = "help_gh",
    label = "GitHub CLI:",
    text = "? Checking…",
  }
  local manager = self.model.managerPackage
  if manager then
    dialog:separator { text = "Manager Updates" }
    if manager.update then
      local update_version = type(manager.update) == "table" and manager.update.version
        or manager.update
      dialog:label {
        label = "Update:",
        text = text(update_version, "Available"),
      }
    end
    if manager.updateError then
      dialog:label {
        label = "Update check:",
        text = Protocol.error_message(manager.updateError),
        hexpand = true,
      }
    elseif not manager.update and self.model.busy then
      dialog:label {
        label = "Update check:",
        text = "Checking…",
      }
    elseif not manager.update then
      dialog:label {
        label = "Update check:",
        text = "Up to date",
      }
    end
    local recovery = recovery_artifact(manager)
    if type(recovery) == "string" and recovery ~= "" then
      dialog:label {
        label = "Recovery package:",
        text = recovery,
        hexpand = true,
      }
    end
    if self_update_available(manager) then
      dialog:button {
        text = "Update Manager…",
        onclick = function()
          dialog:close()
          self.controller:update_package(manager)
        end,
      }
    end
    if manager.rollbackAvailable then
      dialog:button {
        text = "Restore Manager…",
        onclick = function()
          dialog:close()
          self.controller:restore_package(manager)
        end,
      }
    end
  end

  local function update_tool_status(result, error_value)
    if not help_open then
      return
    end
    local tools = type(result) == "table" and result.tools or nil
    local git_status = "? Could not check"
    local gh_status = "? Could not check"
    if error_value == nil and type(tools) == "table" then
      git_status = installed_tool_status(tools.git)
      gh_status = github_cli_status(tools.gh)
    end
    pcall(function()
      dialog:modify {
        id = "help_git",
        text = git_status,
      }
      dialog:modify {
        id = "help_gh",
        text = gh_status,
      }
    end)
  end
  if self.controller and type(self.controller.request_diagnostics) == "function" then
    diagnostics_ticket = self.controller:request_diagnostics(update_tool_status)
    if not diagnostics_ticket then
      update_tool_status(nil, true)
    end
  else
    update_tool_status(nil, true)
  end
  show_responsive(dialog)
end

function Ui:show_preferences()
  local dialog = self.dialog
  if not dialog then
    return false
  end
  dialog:modify {
    id = "startup_checks",
    selected = self.environment.plugin.preferences.startupChecks ~= false,
  }
  self.screen = "preferences"
  self:refresh()
  return true
end

function Ui:show_github_repository_menu(repository)
  if type(repository) ~= "table" then
    return false
  end
  local menu = self.environment.Dialog {
    parent = self:_parent(),
  }
  add_package_menu_info(
    menu,
    "github_repository_identity",
    nil,
    text(repository.nameWithOwner, "Unnamed repository")
  )
  if type(repository.description) == "string" and repository.description ~= "" then
    add_package_menu_info(
      menu,
      "github_repository_description",
      "Description",
      repository.description
    )
  end
  add_package_menu_info(
    menu,
    "github_repository_visibility",
    "Visibility",
    repository.isPrivate and "Private" or "Public"
  )
  if repository.isArchived then
    add_package_menu_info(menu, "github_repository_status", "Status", "Archived")
  elseif repository.isFork then
    add_package_menu_info(menu, "github_repository_status", "Status", "Fork")
  end
  if type(repository.updatedAt) == "string" and repository.updatedAt ~= "" then
    add_package_menu_info(
      menu,
      "github_repository_updated",
      "Updated",
      repository.updatedAt:sub(1, 10)
    )
  end
  if type(repository.viewerPermission) == "string" and repository.viewerPermission ~= "" then
    add_package_menu_info(
      menu,
      "github_repository_permission",
      "Permission",
      repository.viewerPermission
    )
  end
  if not add_repository_menu_action(
    self,
    menu,
    "github_repository_url",
    repository.url
  ) then
    add_package_menu_info(menu, "github_repository_url", "Repository", text(repository.url))
  end
  menu:menuItem {
    id = "github_repository_install",
    text = "Install",
    onclick = function()
      self.controller:install_github_repository(repository)
    end,
  }
  menu:showMenu()
  return true
end

function Ui:show_package_menu(package, kind)
  if type(package) ~= "table" then
    return false
  end
  local manager_package = kind == "installed"
      and Model.is_manager_installed_package(package)
    or Model.is_manager_catalog_package(package)
  local menu = self.environment.Dialog {
    parent = self:_parent(),
  }
  add_package_menu_info(
    menu,
    "package_summary",
    nil,
    package_title(package) .. " · v" .. package_version(package)
  )
  add_package_menu_info(
    menu,
    "package_identity",
    "Package",
    text(package.name, text(package.manifestName, package.id))
  )
  if package.author then
    add_package_menu_info(menu, "package_author", "Author", author_name(package.author))
  end
  if package.license then
    add_package_menu_info(menu, "package_license", "License", text(package.license))
  end

  if kind == "installed" then
    add_package_menu_info(menu, "package_source", "Source", source_label(package.source))
    add_repository_menu_action(
      self,
      menu,
      "package_repository",
      package_repository_url(package)
    )
    add_package_menu_info(
      menu,
      "package_managed",
      "Status",
      package.managed and "✓ Managed" or "⚠ Unmanaged"
    )
    if package.enabled ~= nil then
      add_package_menu_info(
        menu,
        "package_enabled",
        "State",
        package.enabled and "Enabled" or "Disabled"
      )
    end
    if package.update then
      local update_version = type(package.update) == "table" and package.update.version
        or package.update
      add_package_menu_info(
        menu,
        "package_update",
        "Update",
        text(update_version, "Available")
      )
    end
    if package.updateError then
      add_package_menu_info(
        menu,
        "package_update_error",
        "Update check",
        Protocol.error_message(package.updateError)
      )
      local recovery = recovery_artifact(package)
      if type(recovery) == "string" and recovery ~= "" then
        add_package_menu_info(menu, "package_recovery", "Recovery package", recovery)
      end
    end
    local manager_update = self_update_available(package)
    if package.update and (package.managed or manager_update) then
      menu:menuItem {
        id = "package_update_action",
        text = manager_update and "Update Manager…" or "Update",
        onclick = function()
          self.controller:update_package(package)
        end,
      }
    end
    if package.rollbackAvailable then
      menu:menuItem {
        id = "package_restore",
        text = manager_package and "Restore Manager…" or "Restore…",
        onclick = function()
          self.controller:restore_package(package)
        end,
      }
    end
    if not manager_package then
      menu:menuItem {
        id = "package_enable_disable",
        text = "Enable / Disable…",
        onclick = function()
          self.controller:open_native_extension_preferences("enable_disable", package)
        end,
      }
      menu:menuItem {
        id = "package_uninstall",
        text = "Uninstall",
        onclick = function()
          self.controller:uninstall_package(package)
        end,
      }
    end
  else
    if not add_repository_menu_action(
      self,
      menu,
      "package_repository",
      package_repository_url(package)
    ) then
      add_package_menu_info(
        menu,
        "package_repository",
        "Repository",
        text(package.repository)
      )
    end
    if manager_package then
      add_package_menu_info(
        menu,
        "package_availability",
        nil,
        "Manager updates are available from Help."
      )
    elseif not package.latest then
      add_package_menu_info(
        menu,
        "package_availability",
        nil,
        "No compatible stable release is available for this Aseprite version."
      )
    elseif self.model.registryExpired then
      add_package_menu_info(
        menu,
        "package_availability",
        nil,
        "Catalog metadata is expired. This package is view-only."
      )
    else
      menu:menuItem {
        id = "package_install",
        text = "Install",
        onclick = function()
          self.controller:install_registry_package(package)
        end,
      }
    end
  end
  menu:showMenu()
  return true
end

return Ui
