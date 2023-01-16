disablePattern = "^-"
eventPattern = "^*"
commentPattern = "^//"
keyframePattern = "^'"

local function updateTagColors()
	if not (app.activeSprite == nil) then
    local s = app.activeSprite
    for i,tag in ipairs(s.tags) do
        if (tag.name:find(disablePattern) ~= nil) then
            tag.color = Color{ r=255, g=255, b=255, a=100 }		
        elseif (tag.name:find(eventPattern) ~= nil) then
            tag.color = Color{ r=0, g=0, b=180, a=255 }
        elseif (tag.name:find(commentPattern) ~= nil) then
            tag.color = Color{ r=255, g=255, b=255, a=170 }
		elseif (tag.name:find(keyframePattern) ~= nil) then
            tag.color = Color{ r=0, g=255, b=0, a=170 }
		-- Custom Zone start ..
		elseif (tag.name:find("^Damage") ~= nil) then
            tag.color = Color{ r=255, g=0, b=0, a=170 }
		elseif (tag.name:find("^Punch") ~= nil) then
            tag.color = Color{ r=255, g=255, b=0, a=170 }
		-- Custom Zone end ..
        end
    end
end
end

updateTagColors()
app.events:off(updateTagColors)
app.events:on('sitechange', updateTagColors)