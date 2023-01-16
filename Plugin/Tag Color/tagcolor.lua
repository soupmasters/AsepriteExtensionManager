disablePattern = "^-"
eventPattern = "^*"
commentPattern = "^//"
commandPattern = "^@"
keyframePattern = "^'"

local function updateTagColors()
	if not (app.activeSprite == nil) then
    local s = app.activeSprite
    for i,tag in ipairs(s.tags) do
        if (tag.name:find(disablePattern) ~= nil) then
            tag.color = Color{ r=255, g=255, b=255, a=100 }		
        elseif (tag.name:find(eventPattern) ~= nil) then
            tag.color = Color{ r=0, g=0, b=180, a=255 }
        elseif (tag.name:find(commandPattern) ~= nil) then
            tag.color = Color{ r=0, g=0, b=180, a=255 }
        elseif (tag.name:find(commentPattern) ~= nil) then
            tag.color = Color{ r=0, g=0, b=0, a=0 }
		elseif (tag.name:find(keyframePattern) ~= nil) then
            tag.color = Color{ r=0, g=255, b=0, a=170 }
		-- Custom Zone start ..
		elseif (tag.name:find("Damage") ~= nil) then
            tag.color = Color{ r=209, g=134, b=223, a=255 }
		elseif (tag.name:find("Combo") ~= nil) then
            tag.color = Color{ r=209, g=134, b=223, a=255 }
		elseif (tag.name:find("Chip") ~= nil) then
            tag.color = Color{ r=209, g=134, b=223, a=255 }
		elseif (tag.name:find("Stun") ~= nil) then
            tag.color = Color{ r=209, g=134, b=223, a=255 }
		elseif (tag.name:find("Knockout") ~= nil) then
            tag.color = Color{ r=209, g=134, b=223, a=255 }
		elseif (tag.name:find("Crit") ~= nil) then
            tag.color = Color{ r=209, g=134, b=223, a=255 }
			
		elseif (tag.name:find("Punch") ~= nil) then
            tag.color = Color{ r=243, g=206, b=82, a=255 }
		elseif (tag.name:find("Attack") ~= nil) then
            tag.color = Color{ r=255, g=255, b=0, a=255 }
		elseif (tag.name:find("Hook") ~= nil) then
            tag.color = Color{ r=209, g=134, b=223, a=255 }
		elseif (tag.name:find("Jab") ~= nil) then
            tag.color = Color{ r=209, g=134, b=223, a=255 }
		elseif (tag.name:find("Slam") ~= nil) then
            tag.color = Color{ r=209, g=134, b=223, a=255 }
			
		elseif (tag.name:find("Idle") ~= nil) then
            tag.color = Color{ r=106, g=205, b=91, a=255 }
		elseif (tag.name:find("Taunt") ~= nil) then
            tag.color = Color{ r=106, g=205, b=91, a=255 }
		elseif (tag.name:find("Block") ~= nil) then
            tag.color = Color{ r=106, g=205, b=91, a=255 }
		elseif (tag.name:find("Transition") ~= nil) then
            tag.color = Color{ r=106, g=205, b=91, a=255 }
		elseif (tag.name:find("Transformation") ~= nil) then
            tag.color = Color{ r=106, g=205, b=91, a=255 }
		-- Custom Zone end ..
        end
    end
end
end

updateTagColors()
app.events:off(updateTagColors)
app.events:on('sitechange', updateTagColors)