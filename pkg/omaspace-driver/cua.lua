-- omaspace-driver: cua's Hyprland input plugin (cua 0.32.0 + Omarchy remap
-- patch, built for Hyprland 0.56.2). Lets agents type and click into the apps
-- in ~/.config/cua-driver/qualified-apps on their own workspace, without your
-- focus or pointer. Installed by `omaspace setup`; a plugin change needs a
-- Hyprland restart.
hl.plugin.load("/usr/lib/omaspace-driver/cua-hyprland-plugin.so")
hl.config({ plugin = { cua = { enabled = true } } })
