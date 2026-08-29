local Compat = {}

Compat.MINIMUM_API_VERSION = 35
Compat.MINIMUM_ASEPRITE_VERSION = "1.3.15"

local function numeric_components(value, limit)
  local components = {}
  local numeric = tostring(value or ""):match("^(%d+%.%d+%.%d+%.?%d*)") or ""
  for component in numeric:gmatch("%d+") do
    components[#components + 1] = tonumber(component)
    if #components == limit then
      break
    end
  end
  while #components < limit do
    components[#components + 1] = 0
  end
  return components
end

function Compat.compare_versions(left, right)
  local a = numeric_components(left, 4)
  local b = numeric_components(right, 4)
  for index = 1, 4 do
    if a[index] < b[index] then
      return -1
    elseif a[index] > b[index] then
      return 1
    end
  end
  return 0
end

function Compat.release_supported(release, app)
  if type(release) ~= "table" or release.yanked == true then
    return false
  end
  local compatibility = release.aseprite
  if type(compatibility) ~= "table"
    or type(compatibility.minimumVersion) ~= "string"
    or tonumber(compatibility.minimumApi) == nil
  then
    return false
  end
  local current_version = tostring(app and app.version or "0.0.0")
  local current_api = tonumber(app and app.apiVersion) or 0
  if Compat.compare_versions(current_version, compatibility.minimumVersion) < 0
    or current_api < tonumber(compatibility.minimumApi)
  then
    return false
  end
  if compatibility.maximumVersion
    and Compat.compare_versions(current_version, compatibility.maximumVersion) > 0
  then
    return false
  end
  if compatibility.maximumApi
    and current_api > tonumber(compatibility.maximumApi)
  then
    return false
  end
  return tostring(release.version or ""):match("^%d+%.%d+%.%d+$") ~= nil
end

function Compat.select_release(package, app)
  local selected
  for _, release in ipairs(type(package) == "table" and package.releases or {}) do
    if Compat.release_supported(release, app)
      and (not selected
        or Compat.compare_versions(release.version, selected.version) > 0)
    then
      selected = release
    end
  end
  return selected
end

function Compat.check(app)
  if not app or (tonumber(app.apiVersion) or 0) < Compat.MINIMUM_API_VERSION then
    return false,
      "Aseprite Extension Manager requires Aseprite "
        .. Compat.MINIMUM_ASEPRITE_VERSION
        .. " or newer (API "
        .. tostring(Compat.MINIMUM_API_VERSION)
        .. ")."
  end

  if Compat.compare_versions(tostring(app.version or "0"), Compat.MINIMUM_ASEPRITE_VERSION) < 0 then
    return false,
      "Aseprite Extension Manager requires Aseprite "
        .. Compat.MINIMUM_ASEPRITE_VERSION
        .. " or newer."
  end

  return true
end

return Compat
