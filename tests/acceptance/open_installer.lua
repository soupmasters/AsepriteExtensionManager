local package_path = app.params["package"]

if not package_path or package_path == "" then
  error("missing --script-param package=/absolute/path")
end

app.command.Options {
  installExtension = package_path
}
