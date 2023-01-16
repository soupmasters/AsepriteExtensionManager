disablePattern = "^-"
eventPattern = "^*"
commentPattern = "^//"
keyframePattern = "^'"

local function displayAnimationList()

local dlg = Dialog { title = "Animation List" }

if not (app.activeSprite == nil) then
for i,tag in ipairs(app.activeSprite.tags)
do
    if not (tag.name:find(disablePattern) ~= nil) then
        if not (tag.name:find(eventPattern) ~= nil) then
            if not (tag.name:find(commentPattern) ~= nil) then
			if not (tag.name:find(keyframePattern) ~= nil) then
                dlg:button {
                    id = "button" .. i,
                    text = tag.name,
                    onclick = function()
                        app.range:clear()
                        app.command.GotoLastFrame()
                        app.activeFrame = tag.fromFrame
                        app.command.GotoFirstFrameInTag()
                    end
                }
			end
            end
            dlg:newrow() -- write two "--" before dlg:newrow() for horizontal
        end
    end
end
end

dlg:show()

end

function init(plugin)
  plugin:newCommand{
    id="OpenAnimationList",
    title="Open Animation List",
    group="cel_popup_properties",
    onclick=displayAnimationList()
  }
end
