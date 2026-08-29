local Model = {}
Model.__index = Model

local function folded(value)
  return tostring(value or ""):lower()
end

local function package_text(package)
  local source = package.source
  local author = package.author
  if type(source) == "table" then
    source = source.kind or source.repository or source.url or ""
  end
  if type(author) == "table" then
    author = author.name or ""
  end
  return table.concat({
    package.name or "",
    package.manifestName or "",
    package.id or "",
    package.displayName or "",
    author or "",
    package.license or "",
    source or "",
  }, " "):lower()
end

function Model.new(page_size)
  return setmetatable({
    catalog = {},
    installed = {},
    updateErrors = {},
    browseSearch = "",
    installedSearch = "",
    browsePage = 1,
    installedPage = 1,
    pageSize = page_size or 6,
    registryStatus = "bundled",
    registryExpired = false,
    registryFromCache = false,
    status = "Ready",
    busy = false,
  }, Model)
end

function Model:set_catalog(packages, status, expired, from_cache)
  self.catalog = packages or {}
  self.registryStatus = status or self.registryStatus
  self.registryExpired = expired == true
  self.registryFromCache = from_cache == true
  self.browsePage = 1
end

function Model:set_installed(packages)
  self.installed = packages or {}
  self.installedPage = 1
end

function Model:set_update_errors(errors)
  self.updateErrors = errors or {}
end

function Model:set_search(kind, value)
  if kind == "browse" then
    self.browseSearch = value or ""
    self.browsePage = 1
  else
    self.installedSearch = value or ""
    self.installedPage = 1
  end
end

function Model:filtered(kind)
  local list
  local query
  if kind == "browse" then
    list = self.catalog
    query = folded(self.browseSearch)
  else
    list = self.installed
    query = folded(self.installedSearch)
  end

  if query == "" then
    local copy = {}
    for index, package in ipairs(list) do
      copy[index] = package
    end
    return copy
  end

  local result = {}
  for _, package in ipairs(list) do
    if package_text(package):find(query, 1, true) then
      result[#result + 1] = package
    end
  end
  return result
end

function Model:page(kind)
  local filtered = self:filtered(kind)
  local field = kind == "browse" and "browsePage" or "installedPage"
  local page_count = math.max(1, math.ceil(#filtered / self.pageSize))
  self[field] = math.max(1, math.min(self[field], page_count))

  local first = (self[field] - 1) * self.pageSize + 1
  local page = {}
  for index = first, math.min(first + self.pageSize - 1, #filtered) do
    page[#page + 1] = filtered[index]
  end

  return page, self[field], page_count, #filtered
end

function Model:move_page(kind, delta)
  local field = kind == "browse" and "browsePage" or "installedPage"
  self[field] = self[field] + delta
  self:page(kind)
end

return Model
