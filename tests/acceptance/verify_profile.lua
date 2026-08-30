local expected_name = app.params["name"]
local expected_version = app.params["version"]
local extensions_path = app.fs.joinPath(app.fs.userConfigPath, "extensions")
local matches = {}

for _, entry in ipairs(app.fs.listFiles(extensions_path)) do
  local directory = app.fs.joinPath(extensions_path, entry)
  local manifest_path = app.fs.joinPath(directory, "package.json")
  if app.fs.isFile(manifest_path) then
    local file = io.open(manifest_path, "r")
    if file then
      local manifest = json.decode(file:read("*a"))
      file:close()
      if manifest.name == expected_name then
        table.insert(matches, {
          name = manifest.name,
          version = manifest.version,
          path = directory
        })
      end
    end
  end
end

if #matches ~= 1 then
  error("expected exactly one installed extension named " .. expected_name)
end

if expected_version and expected_version ~= "" and
   matches[1].version ~= expected_version then
  error("installed version does not match: " .. tostring(matches[1].version))
end

print(json.encode(matches[1]))
