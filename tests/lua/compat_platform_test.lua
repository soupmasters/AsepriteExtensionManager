local Test = require("testlib")
local Fakes = require("fakes")
local Compat = require("aem.compat")
local Platform = require("aem.platform")

Test.case("minimum supported Aseprite version and API pass", function()
  local app = Fakes.app {
    apiVersion = 35,
    version = "1.3.15",
  }
  local compatible = Compat.check(app)
  Test.truthy(compatible)
end)

Test.case("unsupported API is rejected with a concise message", function()
  local app = Fakes.app {
    apiVersion = 34,
    version = "1.3.18",
  }
  local compatible, message = Compat.check(app)
  Test.falsy(compatible)
  Test.contains(message, "1.3.15")
  Test.contains(message, "API 35")
end)

Test.case("version comparison accepts later patch versions", function()
  Test.equal(Compat.compare_versions("1.3.18.1", "1.3.15"), 1)
  Test.equal(Compat.compare_versions("1.3.18.1-arm64", "1.3.18"), 1)
  Test.equal(Compat.compare_versions("1.3.15", "1.3.15"), 0)
  Test.equal(Compat.compare_versions("1.3.14.4", "1.3.15"), -1)
end)

Test.case("catalog release selection is compatible, stable, and order independent", function()
  local app = Fakes.app {
    apiVersion = 35,
    version = "1.3.18.1-arm64",
  }
  local package = {
    releases = {
      {
        version = "3.0.0",
        yanked = false,
        aseprite = {
          minimumVersion = "1.4.0",
          minimumApi = 35,
        },
      },
      {
        version = "2.0.0",
        yanked = true,
        aseprite = {
          minimumVersion = "1.3.15",
          minimumApi = 35,
        },
      },
      {
        version = "1.5.0",
        yanked = false,
        aseprite = {
          minimumVersion = "1.3.15",
          minimumApi = 35,
        },
      },
      {
        version = "1.9.0",
        yanked = false,
        aseprite = {
          minimumVersion = "1.3.15",
          minimumApi = 35,
        },
      },
    },
  }
  Test.equal(Compat.select_release(package, app).version, "1.9.0")
end)

Test.case("platform helper paths match packaged locations", function()
  local app = Fakes.app {
    allFilesExist = true,
  }
  local mac = Platform.helper_path(app, "/extension")
  Test.equal(mac, "/extension/bin/macos/aem-helper")

  app.os = {
    name = "Windows",
    windows = true,
    x64 = true,
  }
  local windows = Platform.helper_path(app, "C:\\extension")
  Test.equal(windows, "C:\\extension/bin/windows/aem-helper.exe")

  app.os = {
    name = "Linux",
    linux = true,
    x64 = true,
  }
  local linux = Platform.helper_path(app, "/extension")
  Test.equal(linux, "/extension/bin/linux/aem-helper")
end)

Test.case("unknown operating systems are rejected", function()
  local app = Fakes.app {
    allFilesExist = true,
    os = {
      name = "Other",
    },
  }
  local path, message = Platform.helper_path(app, "/extension")
  Test.equal(path, nil)
  Test.contains(message, "not supported")
end)

Test.case("unpackaged architectures are rejected before launch", function()
  local app = Fakes.app {
    allFilesExist = true,
    os = {
      name = "Linux",
      linux = true,
      arm64 = true,
    },
  }
  local path, message = Platform.helper_path(app, "/extension")
  Test.equal(path, nil)
  Test.contains(message, "architecture")
end)

Test.case("Windows shell-sensitive expansion characters are rejected", function()
  local quoted = Platform.shell_quote([[C:\Safe Folder\aem-helper.exe]], "windows")
  Test.equal(quoted, [["C:\Safe Folder\aem-helper.exe"]])

  local percent, percent_error =
    Platform.shell_quote([[C:\%TEMP%\aem-helper.exe]], "windows")
  Test.equal(percent, nil)
  Test.contains(percent_error, "percent")

  local bang, bang_error =
    Platform.shell_quote([[C:\Tools!\aem-helper.exe]], "windows")
  Test.equal(bang, nil)
  Test.contains(bang_error, "exclamation")
end)
