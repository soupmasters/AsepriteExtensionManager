local result = {
  version = tostring(app.version),
  apiVersion = app.apiVersion,
  isUIAvailable = app.isUIAvailable,
  os = app.os,
  userConfigPath = app.fs.userConfigPath
}

print(json.encode(result))
