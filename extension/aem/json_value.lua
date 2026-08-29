local JsonValue = {}

local DEFAULTS = {
  maxDepth = 32,
  maxNodes = 50000,
  maxStringBytes = 1024 * 1024,
  maxTotalStringBytes = 8 * 1024 * 1024,
  maxSerializedBytes = 8 * 1024 * 1024,
}

local function option(options, name)
  local value = options and tonumber(options[name])
  if value and value >= 0 then
    return math.floor(value)
  end
  return DEFAULTS[name]
end

function JsonValue.normalize(value, options)
  local limits = {
    maxDepth = option(options, "maxDepth"),
    maxNodes = option(options, "maxNodes"),
    maxStringBytes = option(options, "maxStringBytes"),
    maxTotalStringBytes = option(options, "maxTotalStringBytes"),
    maxSerializedBytes = option(options, "maxSerializedBytes"),
  }
  local nodes = 0
  local string_bytes = 0
  local active = {}

  local function count_node()
    nodes = nodes + 1
    if nodes > limits.maxNodes then
      return nil, "The decoded message contains too many values."
    end
    return true
  end

  local function count_string(item)
    if #item > limits.maxStringBytes then
      return nil, "The decoded message contains an oversized string."
    end
    string_bytes = string_bytes + #item
    if string_bytes > limits.maxTotalStringBytes then
      return nil, "The decoded message contains too much string data."
    end
    return true
  end

  local normalize
  normalize = function(item, depth)
    if depth > limits.maxDepth then
      return nil, "The decoded message is nested too deeply."
    end
    local counted, count_error = count_node()
    if not counted then
      return nil, count_error
    end

    local item_type = type(item)
    if item_type == "nil" or item_type == "boolean" then
      return item
    elseif item_type == "number" then
      if item ~= item or item == math.huge or item == -math.huge then
        return nil, "The decoded message contains an invalid number."
      end
      return item
    elseif item_type == "string" then
      local valid, string_error = count_string(item)
      if not valid then
        return nil, string_error
      end
      return item
    elseif item_type ~= "table" and item_type ~= "userdata" then
      return nil, "The decoded message contains an unsupported value."
    end

    if active[item] then
      return nil, "The decoded message contains a cycle."
    end
    active[item] = true

    local output = {}
    local failure
    if item_type == "table" then
      local iterated, iterate_error = pcall(function()
        for key, child in pairs(item) do
          local key_type = type(key)
          if key_type ~= "string" and key_type ~= "number" then
            failure = "The decoded message contains an invalid object key."
            break
          end
          if key_type == "string" then
            local valid, key_error = count_string(key)
            if not valid then
              failure = key_error
              break
            end
          elseif key ~= key or key == math.huge or key == -math.huge then
            failure = "The decoded message contains an invalid object key."
            break
          end

          local normalized, child_error = normalize(child, depth + 1)
          if child_error then
            failure = child_error
            break
          end
          output[key] = normalized
        end
      end)
      if not iterated and not failure then
        failure = tostring(iterate_error)
      end
    else
      local rendered_ok, rendered = pcall(tostring, item)
      if not rendered_ok or type(rendered) ~= "string"
        or #rendered > limits.maxSerializedBytes
      then
        failure = "The decoded message contains an invalid JSON value."
      else
        local marker = rendered:match("^%s*(.)")
        if marker == "[" then
          local length_ok, length = pcall(function()
            return #item
          end)
          if not length_ok or type(length) ~= "number"
            or length < 0 or length ~= math.floor(length)
          then
            failure = "The decoded message contains an invalid JSON array."
          elseif length > limits.maxNodes then
            failure = "The decoded message contains too many values."
          else
            for index = 1, length do
              local read_ok, child = pcall(function()
                return item[index]
              end)
              if not read_ok then
                failure = "The decoded message contains an unreadable JSON array."
                break
              end
              if child == nil then
                failure = "JSON null values are not supported inside arrays."
                break
              end
              local normalized, child_error = normalize(child, depth + 1)
              if child_error then
                failure = child_error
                break
              end
              output[index] = normalized
            end
          end
        elseif marker == "{" then
          local iterated, iterate_error = pcall(function()
            for key, child in pairs(item) do
              if type(key) ~= "string" then
                failure = "The decoded message contains an invalid JSON object key."
                break
              end
              local valid, key_error = count_string(key)
              if not valid then
                failure = key_error
                break
              end
              if child ~= nil then
                local normalized, child_error = normalize(child, depth + 1)
                if child_error then
                  failure = child_error
                  break
                end
                output[key] = normalized
              end
            end
          end)
          if not iterated and not failure then
            failure = tostring(iterate_error)
          end
        else
          failure = "The decoded message root must be a JSON object or array."
        end
      end
    end

    active[item] = nil
    if failure then
      return nil, failure
    end
    return output
  end

  return normalize(value, 0)
end

return JsonValue
