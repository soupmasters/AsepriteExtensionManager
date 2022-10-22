disablePattern = "^-"
eventPattern = "^*"
commentPattern = "^//"
---- Animation Tag Jumper
--os.execute("sleep " .. tonumber(0.04))
--app.command.GotoFirstFrameInTag()
local dlg = Dialog { title = "Animation List" }
function getAnimationTags()
    local s = app.activeSprite
    local t = {}
    for i,tag in ipairs(s.tags) do
        if not (tag.name:find(disablePattern) ~= nil) then
            if not (tag.name:find(eventPattern) ~= nil) then
                if not (tag.name:find(commentPattern) ~= nil) then
                    table.insert(t, tag.name) --.. tostring(tag):gsub("Tag:", "")
                end
            end
        end
    end
    return t
end
for i,tag in ipairs(app.activeSprite.tags)
do
    if not (tag.name:find(disablePattern) ~= nil) then
        if not (tag.name:find(eventPattern) ~= nil) then
            if not (tag.name:find(commentPattern) ~= nil) then
                dlg:button {
                    id = "cancel",
                    text = tag.name,
                    onclick = function()
                        app.range:clear()
                        app.command.GotoLastFrame()
                        app.activeFrame = tag.fromFrame
                        --app.range.frames = { tag.fromFrame, ..., tag.toFrame }
                        app.command.GotoFirstFrameInTag()
                    end
                }
            end
            dlg:newrow()
        end
    end
end


dlg:show { wait = false }

---- Tag Color

local function updateTagColors()
    local s = app.activeSprite
    for i,tag in ipairs(s.tags) do
        if (tag.name:find(disablePattern) ~= nil) then
            tag.color = Color{ r=255, g=255, b=255, a=100 }
        elseif (tag.name:find(eventPattern) ~= nil) then
            tag.color = Color{ r=0, g=0, b=180, a=255 }
        elseif (tag.name:find(commentPattern) ~= nil) then
            tag.color = Color{ r=255, g=255, b=255, a=170 }
        else
            tag.color = Color{ r=255, g=255, b=0, a=220 }
        end
    end
end
updateTagColors()