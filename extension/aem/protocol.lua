local Protocol = {}

Protocol.VERSION = 1

Protocol.METHODS = {
  ping = true,
  scanInstalled = true,
  uninstallPackage = true,
  refreshRegistry = true,
  resolveGitHub = true,
  preparePackage = true,
  prepareSelfUpdate = true,
  prepareSelfRollback = true,
  syncLocal = true,
  verifyInstall = true,
  listUpdates = true,
  prepareRollback = true,
  cacheStatus = true,
  clearCache = true,
  diagnostics = true,
  shutdown = true,
}

function Protocol.is_method_allowed(method)
  return Protocol.METHODS[method] == true
end

function Protocol.error_message(value)
  if type(value) == "table" then
    return tostring(value.message or value.code or "The helper returned an error.")
  end
  return tostring(value or "The helper returned an error.")
end

return Protocol
