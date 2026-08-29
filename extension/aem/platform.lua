local Platform = {}

local DEFINITIONS = {
  macos = {
    directory = "macos",
    executable = "aem-helper",
  },
  windows = {
    directory = "windows",
    executable = "aem-helper.exe",
  },
  linux = {
    directory = "linux",
    executable = "aem-helper",
  },
}

function Platform.key(os_info)
  os_info = os_info or {}
  if os_info.macos or os_info.name == "macOS" then
    return "macos"
  elseif os_info.windows or os_info.name == "Windows" then
    return "windows"
  elseif os_info.linux or os_info.name == "Linux" then
    return "linux"
  end
  return nil
end

function Platform.helper_path(app, extension_root)
  local os_info = app and app.os or {}
  local key = Platform.key(os_info)
  local definition = key and DEFINITIONS[key] or nil
  if not definition then
    return nil, "This operating system is not supported."
  end
  if key == "macos" and not os_info.arm64 and not os_info.x64 then
    return nil, "This macOS architecture is not supported."
  end
  if (key == "windows" or key == "linux") and not os_info.x64 then
    return nil, "This " .. tostring(os_info.name or "platform")
      .. " architecture is not supported."
  end

  return app.fs.joinPath(
    extension_root,
    "bin",
    definition.directory,
    definition.executable
  )
end

function Platform.shell_quote(value, platform_key)
  value = tostring(value or "")
  if platform_key == "windows" then
    if value:find('"', 1, true) then
      return nil, "A Windows path cannot contain a double quote."
    end
    if value:find("%%") or value:find("!", 1, true) then
      return nil, "A Windows path cannot contain a percent sign or exclamation mark."
    end
    return '"' .. value .. '"'
  end

  return "'" .. value:gsub("'", [['"'"']]) .. "'"
end

return Platform
