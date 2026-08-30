local Model = {}
Model.__index = Model

local MANAGER_NAME = "aseprite-extension-manager"
Model.MANAGER_NAME = MANAGER_NAME

local BROWSE_SORTS = {
  name_asc = true,
  name_desc = true,
  recent = true,
}
local INSTALLED_SORTS = {
  name_asc = true,
  name_desc = true,
  updates_first = true,
}
local BROWSE_FILTERS = {
  all = true,
  compatible = true,
  unavailable = true,
}
local INSTALLED_FILTERS = {
  all = true,
  updates = true,
  managed = true,
  unmanaged = true,
}

local function folded(value)
  return tostring(value or ""):lower()
end

function Model.is_manager_name(value)
  return type(value) == "string" and folded(value) == MANAGER_NAME
end

function Model.is_manager_catalog_package(package)
  if type(package) ~= "table" then
    return false
  end
  return Model.is_manager_name(package.id)
    and Model.is_manager_name(package.manifestName)
end

function Model.is_manager_installed_package(package)
  return type(package) == "table"
    and package.isSelf == true
    and Model.is_manager_name(package.name)
end

local function partition_manager(packages, predicate)
  local visible = {}
  local manager
  for _, package in ipairs(packages or {}) do
    if predicate(package) then
      manager = manager or package
    else
      visible[#visible + 1] = package
    end
  end
  return visible, manager
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
    package.summary or "",
    author or "",
    package.license or "",
    package.homepage or "",
    package.repository or "",
    source or "",
  }, " "):lower()
end

local function package_sort_name(package)
  return folded(
    package.displayName
      or package.name
      or package.manifestName
      or package.id
      or ""
  )
end

local function package_identity(package)
  return table.concat({
    folded(package.name),
    folded(package.manifestName),
    folded(package.id),
  }, "\0")
end

local function name_before(left, right, descending)
  local left_name = package_sort_name(left)
  local right_name = package_sort_name(right)
  if left_name ~= right_name then
    if descending then
      return left_name > right_name
    end
    return left_name < right_name
  end
  return package_identity(left) < package_identity(right)
end

local function has_update(package)
  return package.update ~= nil and package.update ~= false
end

local function matches_filter(kind, selected, package)
  if selected == "all" then
    return true
  end
  if kind == "browse" then
    local compatible = type(package.latest) == "table"
    if selected == "compatible" then
      return compatible
    end
    return not compatible
  end
  if selected == "updates" then
    return has_update(package)
  elseif selected == "managed" then
    return package.managed == true
  end
  return package.managed ~= true
end

local function sort_packages(kind, selected, packages)
  table.sort(packages, function(left, right)
    if kind == "browse" and selected == "recent" then
      local left_date = type(left.latest) == "table"
          and tostring(left.latest.publishedAt or "")
        or ""
      local right_date = type(right.latest) == "table"
          and tostring(right.latest.publishedAt or "")
        or ""
      if left_date ~= right_date then
        return left_date > right_date
      end
    elseif kind == "installed" and selected == "updates_first" then
      local left_update = has_update(left)
      local right_update = has_update(right)
      if left_update ~= right_update then
        return left_update
      end
    end
    return name_before(left, right, selected == "name_desc")
  end)
end

function Model.new(page_size, browse_page_size)
  return setmetatable({
    catalog = {},
    installed = {},
    githubRepositories = {},
    updateErrors = {},
    managerCatalogPackage = nil,
    managerPackage = nil,
    browseSearch = "",
    installedSearch = "",
    githubSearch = "",
    browseSort = "name_asc",
    installedSort = "name_asc",
    browseFilter = "all",
    installedFilter = "all",
    browsePage = 1,
    installedPage = 1,
    githubPage = 1,
    githubPageCount = 1,
    githubTotal = 0,
    githubHasNextPage = false,
    githubEndCursor = nil,
    githubLoading = false,
    githubLoaded = false,
    githubError = nil,
    pageSize = page_size or 6,
    browsePageSize = browse_page_size or page_size or 6,
    registryStatus = "bundled",
    registryExpired = false,
    registryFromCache = false,
    status = "Ready",
    busy = false,
  }, Model)
end

function Model:set_catalog(packages, status, expired, from_cache)
  self.catalog, self.managerCatalogPackage = partition_manager(
    packages,
    Model.is_manager_catalog_package
  )
  self.registryStatus = status or self.registryStatus
  self.registryExpired = expired == true
  self.registryFromCache = from_cache == true
  self.browsePage = 1
end

function Model:set_installed(packages)
  self.installed, self.managerPackage = partition_manager(
    packages,
    Model.is_manager_installed_package
  )
  self.installedPage = 1
end

function Model:set_github_page(packages, page, total, has_next_page, end_cursor)
  self.githubRepositories = packages or {}
  self.githubPage = math.max(1, tonumber(page) or 1)
  self.githubHasNextPage = has_next_page == true
  local page_offset = (self.githubPage - 1) * self.pageSize
  local reported = math.max(0, tonumber(total) or #self.githubRepositories)
  self.githubTotal = page_offset + math.max(#self.githubRepositories, reported)
  self.githubPageCount = self.githubHasNextPage
      and self.githubPage + 1
    or self.githubPage
  self.githubEndCursor = type(end_cursor) == "string" and end_cursor or nil
  self.githubLoading = false
  self.githubLoaded = true
  self.githubError = nil
end

function Model:set_github_error(error_value)
  self.githubRepositories = {}
  self.githubPage = 1
  self.githubPageCount = 1
  self.githubTotal = 0
  self.githubHasNextPage = false
  self.githubEndCursor = nil
  self.githubLoading = false
  self.githubLoaded = true
  self.githubError = error_value
end

function Model:set_update_errors(errors)
  local visible = {}
  for _, update_error in ipairs(errors or {}) do
    if not Model.is_manager_name(update_error.packageName)
      and not Model.is_manager_installed_package(update_error)
    then
      visible[#visible + 1] = update_error
    end
  end
  self.updateErrors = visible
end

function Model:set_search(kind, value)
  if kind == "browse" then
    self.browseSearch = value or ""
    self.browsePage = 1
    return true
  elseif kind == "installed" then
    self.installedSearch = value or ""
    self.installedPage = 1
    return true
  elseif kind == "github" then
    self.githubSearch = value or ""
    self.githubPage = 1
    return true
  end
  return false
end

function Model:set_sort(kind, value)
  if kind == "browse" then
    if not BROWSE_SORTS[value] then
      return false
    end
    if self.browseSort == value then
      return false
    end
    self.browseSort = value
    self.browsePage = 1
    return true
  elseif kind == "installed" then
    if not INSTALLED_SORTS[value] then
      return false
    end
    if self.installedSort == value then
      return false
    end
    self.installedSort = value
    self.installedPage = 1
    return true
  end
  return false
end

function Model:set_filter(kind, value)
  if kind == "browse" then
    if not BROWSE_FILTERS[value] then
      return false
    end
    if self.browseFilter == value then
      return false
    end
    self.browseFilter = value
    self.browsePage = 1
    return true
  elseif kind == "installed" then
    if not INSTALLED_FILTERS[value] then
      return false
    end
    if self.installedFilter == value then
      return false
    end
    self.installedFilter = value
    self.installedPage = 1
    return true
  end
  return false
end

function Model:filtered(kind)
  local list
  local query
  local selected_filter
  local selected_sort
  if kind == "browse" then
    list = self.catalog
    query = folded(self.browseSearch)
    selected_filter = self.browseFilter
    selected_sort = self.browseSort
  elseif kind == "installed" then
    list = self.installed
    query = folded(self.installedSearch)
    selected_filter = self.installedFilter
    selected_sort = self.installedSort
  else
    return {}
  end

  local result = {}
  for _, package in ipairs(list) do
    local search_match = query == "" or package_text(package):find(query, 1, true)
    if search_match and matches_filter(kind, selected_filter, package) then
      result[#result + 1] = package
    end
  end
  sort_packages(kind, selected_sort, result)
  return result
end

function Model:page(kind)
  if kind == "github" then
    return self.githubRepositories,
      self.githubPage,
      self.githubPageCount,
      self.githubTotal
  end
  local filtered = self:filtered(kind)
  local field
  local page_size = self.pageSize
  if kind == "browse" then
    field = "browsePage"
    page_size = self.browsePageSize
  elseif kind == "installed" then
    field = "installedPage"
  else
    return {}, 1, 1, 0
  end
  local page_count = math.max(1, math.ceil(#filtered / page_size))
  self[field] = math.max(1, math.min(self[field], page_count))

  local first = (self[field] - 1) * page_size + 1
  local page = {}
  for index = first, math.min(first + page_size - 1, #filtered) do
    page[#page + 1] = filtered[index]
  end

  return page, self[field], page_count, #filtered
end

function Model:move_page(kind, delta)
  local field
  if kind == "browse" then
    field = "browsePage"
  elseif kind == "installed" then
    field = "installedPage"
  else
    return false
  end
  self[field] = self[field] + delta
  self:page(kind)
  return true
end

return Model
