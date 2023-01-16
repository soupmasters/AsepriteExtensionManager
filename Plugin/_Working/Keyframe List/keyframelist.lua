keyframePattern = "^'"

local function displayKeyframeList()
local dlg = Dialog { title = "Keyframe List" }

if not (app.activeSprite == nil) then
for i,tag in ipairs(app.activeSprite.tags)
do
    if (tag.name:find(keyframePattern) ~= nil) then
        dlg:button {
            id = "button: " .. i,
            text = "Frame: " .. i,	-- Ludvig om du vill ändra på vad som står på knappen så gör det här, (ta bara inte bort kommatecknet i slutet.)
            onclick = function()
                app.range:clear()
                app.command.GotoLastFrame()
                app.activeFrame = tag.fromFrame
                app.command.GotoFirstFrameInTag()
                end
        }
        end
    --dlg:newrow()	-- remove the two "--" before dlg:newrow() for vertical list.
end
end

dlg:show()

end


function init(plugin)
  plugin:newCommand{
    id="OpenKeyframeList",
    title="Open Keyframe List",
    group="cel_popup_properties",
    onclick=function()
      displayKeyframeList()
    end
  }
end
