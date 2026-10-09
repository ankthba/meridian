# dmgbuild settings for Meridian's disk image, used by scripts/release.sh.
# Defines (-D): app = path to Meridian.app, background = path to
# background.png (dmgbuild adds background@2x.png beside it for Retina).
# The background is drawn by scripts/make-dmg-background.swift; icon
# positions here must match appCenter/linkCenter there.
import os.path

app = defines["app"]  # noqa: F821 (provided by dmgbuild)
name = os.path.basename(app)

format = "UDZO"
filesystem = "HFS+"
files = [app]
symlinks = {"Applications": "/Applications"}
icon = os.path.join(app, "Contents", "Resources", "AppIcon.icns")
background = defines["background"]  # noqa: F821

window_rect = ((200, 140), (660, 400))
default_view = "icon-view"
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
show_icon_preview = False
show_item_info = False
arrange_by = None
label_pos = "bottom"
icon_size = 112
text_size = 13
icon_locations = {name: (170, 190), "Applications": (490, 190)}
