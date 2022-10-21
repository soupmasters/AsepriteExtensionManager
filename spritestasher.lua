local spriteStashLocation = "/Users/martin/Dropbox/[ Big Boy Boxing ]/Artwork/Characters/TestScriptStasherLALALA/" -- "/Users/martin/Dropbox/[ Big Boy Boxing ]/Artwork/Characters/TestScriptStasherLALALA/"
local extension = ".ase" --.ase or .aseprite

function sendToSpriteStash()
    newLocation = spriteStashLocation .. app.fs.fileTitle(app.activeSprite.filename)
    if not app.fs.isFile(newLocation .. extension) then
        app.activeSprite:saveCopyAs(newLocation .. extension)
    else
        for i = 1,1000,1
        do
            if not app.fs.isFile(newLocation .. " (" .. tostring(i) .. ")" .. extension) then
                app.activeSprite:saveCopyAs(newLocation .. " (" .. tostring(i) .. ")" .. extension)
                return
            end
        end
    end
end

sendToSpriteStash()