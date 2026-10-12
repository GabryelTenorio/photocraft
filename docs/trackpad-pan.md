# Canvas trackpad panning

For #1989, canvas pan applies the arriving wheel delta without a second smoothing
pass. OS inertia still arrives as wheel events. Point, Line and Page units and
Shift/Ctrl/Command horizontal scrolling retain their conversions.

Rebased over #2577: Zoom with Trackpad Pinch still gates pinch zoom, including
Windows Ctrl-wheel folding. Zoom with Scroll Wheel still swaps ordinary and Alt
wheel behavior. A zero-delta End packet cannot change the gesture's Alt intent.
The regression tests cover arriving-frame movement, no delayed tail, End packets,
modifier directions, and the existing pinch/zoom preferences.
