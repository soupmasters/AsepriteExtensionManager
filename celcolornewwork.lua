---@diagnostic disable: undefined-global
function showEventDialog(cels)
  -- Assume single cel if not a table
  if not cels then
    app.alert("No cel(s) selected.")
    return
  end
  if #cels == 0 then
    app.alert("No cel(s) found at this frame/layer.")
    return
  end

  local existing = cels[1].data.event or ""
  local dlg = Dialog("Add Event")
  dlg:entry{ id="event", label="Event", text=existing }
  dlg:button{ id="ok", text="OK", onclick=function()
    local input = dlg.data.event
    for _, cel in ipairs(cels) do
      cel.data = input
    end
    dlg:close()
  end}
  dlg:button{ text="Cancel", onclick=function() dlg:close() end }
  dlg:show()
end

function AddEvent()
  local sprite = app.activeSprite
  if not sprite then
    app.alert("No active sprite.")
    return
  end

  local cel = app.activeCel
  local frame = app.activeFrame
  local layer = app.activeLayer

  if cel then
    -- Context: Cel
    showEventDialog({ cel })
  elseif frame and layer then
    -- Context: Frame (collect all cels in this frame across visible layers)
    local celsOnFrame = {}
    for _, lyr in ipairs(sprite.layers) do
      if lyr.isImage and not lyr.isGroup then
        local cel = lyr:cel(frame)
        if cel then
          table.insert(celsOnFrame, cel)
        end
      end
    end
    showEventDialog(celsOnFrame)
  else
    app.alert("No cel or frame selected.")
  end
end

function init(plugin)
  print("AddEvent Plugin Initialized")

  -- Right-click on cel
  plugin:newCommand{
    id = "addEventCommandCel",
    title = "Add Event",
    group = "cel_popup_new",
    onclick = AddEvent
  }

  -- Right-click on frame
  plugin:newCommand{
    id = "addEventCommandFrame",
    title = "Add Event",
    group = "frame_popup_reverse",
    onclick = AddEvent
  }
end

function exit(plugin)
  print("AddEvent Plugin Exited")
end

EventLayerName = "@Events"

GreyColor = Color{ r=212, g=211, b=211, a=255 }
RedColor = Color{ r=255, g=0, b=0, a=255 }
BlackColor = Color{ r=255, g=255, b=255, a=255 }
DefaultColor = Color{ r=255, g=255, b=255, a=0 }

function string.starts(String,Start)
    return string.sub(String,1,string.len(Start))==Start
end

function OnSelectCel(ev)
    if(ev.fromUndo) then
        return
    end
    if app.site.cel == nil then
        if app.site.layer.name == EventLayerName then
            app.sprite:newCel(app.site.layer, app.site.frameNumber)
            if app.site.cel ~= nil then
                app.site.cel.data = ""
            end
        end
    end
    DoCel(app.site.cel)
end

function DoCel(cel)
    if cel == nil then
        return
    end
    if cel.data == ""  then
        if cel.color == RedColor then
            cel.color = DefaultColor
        end
    else
        if string.starts(cel.data, "event:@") == false then
            cel.data = "event:@" .. cel.data
        end
        if cel.color ~= RedColor then
            cel.color = RedColor
        end
    end
end

--app.events:on('sitechange', OnSelectCel)

PreviousSprite = nil
app.events:on('sitechange',
        function()
            if PreviousSprite ~= app.sprite then
                --print("new")
                if PreviousSprite ~= nil then
                    PreviousSprite.events:off(OnSelectCel)
                end
                PreviousSprite = app.sprite
                if app.sprite ~= nil then
                    local layerTo = nil
                    for i,layer in ipairs(app.sprite.layers) do
                        if layer.layers ~= nil then
                            for i,layer in ipairs(layer.layers) do
                                if layer.name == EventLayerName then
                                    layerTo = layer
                                    layer.isEditable = false
                                    layer.isVisible = true
                                end
                            end
                        elseif layer.name == EventLayerName then
                            layerTo = layer
                            layer.isEditable = false
                            layer.isVisible = true
                        end
                    end
                    if layerTo ~= nil then
                        for i,frame in ipairs(app.sprite.frames) do
                            app.sprite:newCel(layerTo, i)
                        end
                    end
                    app.sprite.events:on('change', OnSelectCel)
                end

            end


        end)



local sprite = app.activeSprite
app.transaction(function()
  for i, frame in ipairs(sprite.frames) do
    local tag = sprite:newTag(i, i)
    tag.name = "F" .. i
    tag.color = Color{ r=math.random(255), g=math.random(255), b=math.random(255) }
  end
end)