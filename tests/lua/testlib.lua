local Test = {
  cases = {},
}

local function printable(value)
  if type(value) == "string" then
    return string.format("%q", value)
  end
  return tostring(value)
end

function Test.case(name, callback)
  Test.cases[#Test.cases + 1] = {
    name = name,
    callback = callback,
  }
end

function Test.equal(actual, expected, message)
  if actual ~= expected then
    error(
      (message and message .. ": " or "")
        .. "expected "
        .. printable(expected)
        .. ", got "
        .. printable(actual),
      2
    )
  end
end

function Test.truthy(value, message)
  if not value then
    error(message or "expected a truthy value", 2)
  end
end

function Test.falsy(value, message)
  if value then
    error(message or "expected a falsy value", 2)
  end
end

function Test.contains(value, fragment, message)
  if not tostring(value):find(fragment, 1, true) then
    error(
      message
        or ("expected " .. printable(value) .. " to contain " .. printable(fragment)),
      2
    )
  end
end

function Test.run()
  local failures = {}
  for _, case in ipairs(Test.cases) do
    local ok, failure = xpcall(case.callback, debug.traceback)
    if ok then
      print("PASS " .. case.name)
    else
      failures[#failures + 1] = case.name .. "\n" .. tostring(failure)
      print("FAIL " .. case.name)
    end
  end

  print(
    tostring(#Test.cases - #failures)
      .. " passed, "
      .. tostring(#failures)
      .. " failed"
  )
  if #failures > 0 then
    error(table.concat(failures, "\n\n"), 0)
  end
end

return Test
