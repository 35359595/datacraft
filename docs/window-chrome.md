# Window header

Windows uses one application-drawn header. The project menu and data-source
selector sit on the left; search, settings and window controls sit on the right.
There is no second system caption or unused traffic-light gap on Windows.

Minimize, maximize/restore and close dispatch egui's native viewport commands.
Dragging the empty header moves the window; double-clicking it toggles maximization.
Buttons and menus are excluded from that drag region. Thin edge and corner hit
regions dispatch native resize commands while the window is neither maximized nor
fullscreen.

macOS retains native decorations, traffic lights and full-size content. Only macOS
reserves space for traffic lights, independently of the application's zoom; that
space is removed in fullscreen. Custom Windows buttons and resize regions are not
installed on macOS.

Windows checks cover caption commands, resize hit regions and the platform layout
policy. A release-build smoke test verified minimize, maximize/restore, header
double-click and close with a disposable SQLite database. Automated drag gestures
did not confirm native moving or resizing, so those need a manual check. Native
macOS window behavior also needs verification on macOS.
