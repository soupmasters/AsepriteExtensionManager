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
  return package_title(package) .. "  v" .. package_version(package)
end

local function narrow_installed_row(package)
  return table.concat({
    package_title(package) .. "  v" .. package_version(package),
    package.managed and "✓" or "⚠",
  }, " · ")
end

local function show_responsive(dialog, options)
  options = options or {}
  options.autoscrollbars = true
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

  local function add_combobox(field, id, visible, label)
    local choices = options[field]
    dialog:combobox {
      id = id,
      label = label,
      options = option_labels(choices),
      option = option_label(choices, view_value(ui.model, kind, field)),
      visible = visible,
      hexpand = true,
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
    dialog:label {
      id = kind .. "_filter_label",
      text = "Filter:",
      hexpand = false,
    }
    dialog:samerow()
    add_combobox("filter", kind .. "_filter", true)
    dialog:samerow()
    dialog:label {
      id = kind .. "_sort_label",
      text = "Sort:",
      hexpand = false,
    }
    dialog:samerow()
    add_combobox("sort", kind .. "_sort", true)
  else
    add_combobox("filter", kind .. "_filter", true, "Filter:")
    add_combobox("sort", kind .. "_sort", true, "Sort:")
  end
  dialog:newrow()

  if supports_same_row then
    add_combobox("filter", kind .. "_stacked_filter", false, "Filter:")
    dialog:newrow()
    add_combobox("sort", kind .. "_stacked_sort", false, "Sort:")
    dialog:newrow()
  end
end

local function add_pager(ui, dialog, kind, supports_same_row)
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
    }
    stay_on_row()
  end
  dialog:button {
    id = kind .. "_previous",
    text = "←",
    focus = false,
    hexpand = false,
    onclick = function()
      ui.model:move_page(kind, -1)
      ui:refresh()
    end,
  }
  stay_on_row()
  if supports_same_row then
    dialog:label {
      id = kind .. "_page",
      text = "1 / 1",
      hexpand = false,
    }
  else
    dialog:button {
      id = kind .. "_page",
      text = "1 / 1",
      enabled = false,
      focus = false,
      hexpand = false,
    }
  end
  stay_on_row()
  dialog:button {
    id = kind .. "_next",
    text = "→",
    focus = false,
    hexpand = false,
    onclick = function()
      ui.model:move_page(kind, 1)
      ui:refresh()
    end,
  }
  if supports_same_row then
    stay_on_row()
    dialog:label {
      id = kind .. "_pager_right_spacer",
      text = "",
      hexpand = true,
    }
  end
end

local function update_pager(dialog, kind, page, pages, busy, supports_same_row)
  local visible = pages > 1
  dialog:modify {
    id = kind .. "_page",
    text = tostring(page) .. " / " .. tostring(pages),
    visible = visible,
  }
  dialog:modify {
    id = kind .. "_previous",
    visible = visible,
    enabled = visible and page > 1 and not busy,
  }
  dialog:modify {
    id = kind .. "_next",
    visible = visible,
    enabled = visible and page < pages and not busy,
  }
  if supports_same_row then
    dialog:modify {
      id = kind .. "_pager_left_spacer",
      visible = visible,
    }
    dialog:modify {
      id = kind .. "_pager_right_spacer",
      visible = visible,
    }
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
    suppressClose = false,
    supportsSameRow = false,
    responsiveReady = false,
    rowsStacked = false,
    wideRowMinWidth = nil,
    layoutTimer = nil,
    pendingLayoutWidth = nil,
  }, Ui)
end

function Ui:_parent()
  return self.dialog
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
  local scale = tonumber(self.app and self.app.uiScale) or 1
  return FALLBACK_INLINE_WIDTH * math.max(1, scale)
end

function Ui:_capture_wide_row_width()
  if not self.supportsSameRow or self.rowsStacked or not self.dialog then
    return
  end
  local candidate = math.max(
    dialog_dimension(self.dialog, "sizeHint", "width") or 0,
    self:_fallback_inline_width()
  )
  self.wideRowMinWidth = math.max(self.wideRowMinWidth or 0, candidate)
end

function Ui:_row_package(kind, index)
  if kind == "browse" then
    return self.browseRows[index]
  end
  return self.installedRows[index]
end

function Ui:_row_text(kind, package)
  if not package then
    return ""
  end
  if self.supportsSameRow and self.rowsStacked then
    if kind == "browse" then
      return narrow_catalog_row(package)
    end
    return narrow_installed_row(package)
  end
  if kind == "browse" then
    return catalog_row(package)
  end
  return installed_row(package)
end

function Ui:_update_row_controls(kind, index)
  local dialog = self.dialog
  if not dialog then
    return
  end
  local package = self:_row_package(kind, index)
  local present = package ~= nil
  local stacked = self.supportsSameRow and self.rowsStacked
  dialog:modify {
    id = kind .. "_row_" .. tostring(index),
    text = self:_row_text(kind, package),
    visible = present,
  }
  dialog:modify {
    id = kind .. "_details_" .. tostring(index),
    visible = present and not stacked,
    enabled = not self.model.busy,
  }
  if self.supportsSameRow then
    dialog:modify {
      id = kind .. "_stacked_details_" .. tostring(index),
      visible = present and stacked,
      enabled = not self.model.busy,
    }
  end
end

function Ui:_update_view_controls(kind)
  local dialog = self.dialog
  if not dialog then
    return
  end
  local stacked = self.supportsSameRow and self.rowsStacked
  local options = VIEW_OPTIONS[kind]
  if self.supportsSameRow then
    dialog:modify {
      id = kind .. "_filter_label",
      visible = not stacked,
    }
    dialog:modify {
      id = kind .. "_sort_label",
      visible = not stacked,
    }
  end
  for _, field in ipairs({ "filter", "sort" }) do
    local selected = option_label(options[field], view_value(self.model, kind, field))
    dialog:modify {
      id = kind .. "_" .. field,
      option = selected,
      visible = not stacked,
    }
    if self.supportsSameRow then
      dialog:modify {
        id = kind .. "_stacked_" .. field,
        option = selected,
        visible = stacked,
      }
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
  for _, kind in ipairs({ "browse", "installed" }) do
    self:_update_view_controls(kind)
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
      if self.dialog == dialog and pending_width then
        self:_apply_layout_width(pending_width)
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
    text = "Install public GitHub extensions and link local development folders.",
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

  dialog:tab {
    id = "browse_tab",
    text = "Browse",
  }
  dialog:entry {
    id = "browse_search",
    label = "Search:",
    text = self.model.browseSearch,
    hexpand = true,
    onchange = function()
      self.model:set_search("browse", dialog.data.browse_search)
      self:refresh()
    end,
  }
  add_view_controls(self, dialog, "browse", self.supportsSameRow)
  dialog:label {
    id = "browse_empty",
    text = "",
    hexpand = true,
  }
  for index = 1, self.model.pageSize do
    local row_index = index
    dialog:label {
      id = "browse_row_" .. tostring(index),
      text = "",
      visible = false,
      hexpand = true,
    }
    if self.supportsSameRow then
      dialog:samerow()
    end
    dialog:button {
      id = "browse_details_" .. tostring(index),
      text = "Details",
      visible = false,
      hexpand = false,
      onclick = function()
        local package = self.browseRows[row_index]
        if package then
          self:show_package_details(package, "browse")
        end
      end,
    }
    dialog:newrow()
    if self.supportsSameRow then
      dialog:button {
        id = "browse_stacked_details_" .. tostring(index),
        text = "Details",
        visible = false,
        hexpand = false,
        onclick = function()
          local package = self.browseRows[row_index]
          if package then
            self:show_package_details(package, "browse")
          end
        end,
      }
      dialog:newrow()
    end
  end
  add_pager(self, dialog, "browse", self.supportsSameRow)

  dialog:tab {
    id = "installed_tab",
    text = "Installed",
  }
  dialog:entry {
    id = "installed_search",
    label = "Search:",
    text = self.model.installedSearch,
    hexpand = true,
    onchange = function()
      self.model:set_search("installed", dialog.data.installed_search)
      self:refresh()
    end,
  }
  add_view_controls(self, dialog, "installed", self.supportsSameRow)
  dialog:label {
    id = "installed_empty",
    text = "No user extensions were found.",
    hexpand = true,
  }
  for index = 1, self.model.pageSize do
    local row_index = index
    dialog:label {
      id = "installed_row_" .. tostring(index),
      text = "",
      visible = false,
      hexpand = true,
    }
    if self.supportsSameRow then
      dialog:samerow()
    end
    dialog:button {
      id = "installed_details_" .. tostring(index),
      text = "Details",
      visible = false,
      hexpand = false,
      onclick = function()
        local package = self.installedRows[row_index]
        if package then
          self:show_package_details(package, "installed")
        end
      end,
    }
    dialog:newrow()
    if self.supportsSameRow then
      dialog:button {
        id = "installed_stacked_details_" .. tostring(index),
        text = "Details",
        visible = false,
        hexpand = false,
        onclick = function()
          local package = self.installedRows[row_index]
          if package then
            self:show_package_details(package, "installed")
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
  }

  dialog:separator {}
  dialog:label {
    id = "manager_status",
    text = self.model.status,
    hexpand = true,
  }
  dialog:newrow()
  local compact_footer = self.supportsSameRow
  if compact_footer then
    dialog:label {
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
  })
  pcall(function()
    self.dialog.autofit = WIDTH_PRESERVING_AUTOFIT
  end)
  self:_schedule_row_layout(self.dialog)
end

function Ui:close()
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
    visible = empty_message ~= "",
  }
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
    self.supportsSameRow
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
    visible = installed_empty,
  }
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
    self.supportsSameRow
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
    visible = manager_update,
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
  dialog:label {
    text = message,
    hexpand = true,
  }
  dialog:button {
    id = "confirm_action",
    text = confirm_text or "Continue",
    focus = true,
  }
  dialog:button {
    id = "cancel_action",
    text = cancel_text or "Cancel",
  }
  show_responsive(dialog)
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
    text = "Enter a public GitHub repository URL or",
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
  local dialog = self.environment.Dialog {
    title = "About Aseprite Extension Manager",
    parent = self:_parent(),
    resizeable = true,
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
  show_responsive(dialog)
end

function Ui:show_preferences()
  local preferences = self.environment.plugin.preferences
  local dialog = self.environment.Dialog {
    title = "Extension Manager Preferences",
    parent = self:_parent(),
    resizeable = true,
  }
  dialog:check {
    id = "startup_checks",
    text = "Check for updates at startup",
    selected = preferences.startupChecks ~= false,
  }
  dialog:label {
    text = "Checks linked sources and manager releases at most once every 24 hours.",
    hexpand = true,
  }
  dialog:separator { text = "Local Data" }
  dialog:label {
    text = self.app.fs.joinPath(self.app.fs.userConfigPath, "extension-manager"),
    hexpand = true,
  }
  dialog:button {
    text = "Clear Cache…",
    onclick = function()
      if self:confirm(
        "Clear Download Cache",
        "Remove cached package artifacts that are not needed for the current restore point?",
        "Clear Cache"
      ) then
        dialog:close()
        self.controller:clear_cache()
      end
    end,
  }
  dialog:newrow()
  dialog:button {
    id = "save_preferences",
    text = "Save",
    focus = true,
  }
  dialog:button {
    id = "cancel_preferences",
    text = "Discard",
  }
  show_responsive(dialog)
  if dialog.data.save_preferences then
    preferences.startupChecks = dialog.data.startup_checks == true
  end
end

function Ui:show_package_details(package, kind)
  local manager_package = kind == "installed"
      and Model.is_manager_installed_package(package)
    or Model.is_manager_catalog_package(package)
  local dialog = self.environment.Dialog {
    title = package_title(package),
    parent = self:_parent(),
    resizeable = true,
  }
  dialog:label {
    label = "Package:",
    text = text(package.name, text(package.manifestName, package.id)),
    hexpand = true,
  }
  dialog:label {
    label = "Version:",
    text = package_version(package),
  }
  if package.author then
    dialog:label {
      label = "Author:",
      text = author_name(package.author),
    }
  end
  if package.license then
    dialog:label {
      label = "License:",
      text = text(package.license),
    }
  end

  if kind == "installed" then
    dialog:label {
      label = "Source:",
      text = source_label(package.source),
      hexpand = true,
    }
    dialog:label {
      label = "Managed:",
      text = package.managed and "Yes" or "No",
    }
    if package.enabled ~= nil then
      dialog:label {
        label = "Enabled:",
        text = package.enabled and "Yes" or "No",
      }
    end
    if package.update then
      local update_version = type(package.update) == "table" and package.update.version
        or package.update
      dialog:label {
        label = "Update:",
        text = text(update_version, "Available"),
      }
    end
    if package.updateError then
      dialog:label {
        label = "Update check:",
        text = Protocol.error_message(package.updateError),
        hexpand = true,
      }
      local recovery = recovery_artifact(package)
      if type(recovery) == "string" and recovery ~= "" then
        dialog:label {
          label = "Recovery package:",
          text = recovery,
          hexpand = true,
        }
      end
    end
    dialog:separator {}
    local manager_update = self_update_available(package)
    if package.update and (package.managed or manager_update) then
      dialog:button {
        text = manager_update and "Update Manager…" or "Update",
        onclick = function()
          dialog:close()
          self.controller:update_package(package)
        end,
      }
    end
    if package.rollbackAvailable then
      dialog:button {
        text = manager_package and "Restore Manager…" or "Restore…",
        onclick = function()
          dialog:close()
          self.controller:restore_package(package)
        end,
      }
    end
    if not manager_package then
      dialog:button {
        text = "Enable / Disable…",
        onclick = function()
          dialog:close()
          self.controller:open_native_extension_preferences("enable_disable", package)
        end,
      }
      dialog:button {
        text = "Uninstall…",
        onclick = function()
          dialog:close()
          self.controller:open_native_extension_preferences("uninstall", package)
        end,
      }
    end
  else
    dialog:label {
      label = "Repository:",
      text = text(package.repository),
      hexpand = true,
    }
    if manager_package then
      dialog:label {
        text = "Manager updates are available from Help.",
      }
    elseif not package.latest then
      dialog:label {
        text = "No compatible stable release is available for this Aseprite version.",
      }
    elseif self.model.registryExpired then
      dialog:label {
        text = "Catalog metadata is expired. This package is view-only.",
      }
    else
      dialog:separator {}
      dialog:button {
        text = "Install",
        onclick = function()
          dialog:close()
          self.controller:install_registry_package(package)
        end,
      }
    end
  end
  dialog:button {
    text = "Close",
  }
  show_responsive(dialog)
end

return Ui
