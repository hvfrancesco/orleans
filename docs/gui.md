# GUI guide

`orleans gui [design.svg|design.orleans]` opens the visual editor. The input is
optional: start with an empty document and import or draw geometry later.

## Canvas navigation and selection

- Scroll the wheel to zoom at the cursor.
- Right-drag or middle-drag to pan.
- Press `F` or double-click to fit the design.
- Drag on empty canvas from left to right to select fully enclosed shapes.
  Drag right to left to select intersecting shapes.
- Hold `Ctrl` or `Shift` while clicking to add to or remove from the selection.
- In node-editing mode, the same directional window selects nodes. The overlay
  is blue for containment and amber for crossing selection.
- Selected shapes are highlighted with their bounds and a group box. Drag any
  selected shape to move the whole selection.
- Drag a corner handle to scale uniformly around the opposite corner. Drag the
  circular handle above the box to rotate around the selection centre. Hold
  `Ctrl` while rotating to snap to 15° increments.

Millimetre rulers run along the left and bottom edges of the canvas. They share
the canvas world origin and pan and zoom with the drawing.

The **theme** dropdown switches between dark and light interfaces. The choice
applies immediately and lasts for the session. Canvas background, grid, page,
bed, travel, handles and node markers follow the theme.

The **Language** menu switches between English, Italiano, Français, Español,
Deutsch and 简体中文. The choice is saved in the user configuration directory and
restored at startup. A system CJK fallback font is loaded at startup so Chinese
text is readable before switching languages.

The **Help** menu contains **About orleans**, which shows the running version,
author, license and icon attribution.

## Layers

Every SVG colour becomes a layer when a design is opened. The left panel is a
compact, scrollable layer list. Select a layer to edit its name, colour, speed,
power and passes.

**Cut** decides whether the layer is planned, framed and sent. **Show** controls
whether it is drawn. The two flags are independent:

- A visible layer with Cut disabled remains on the canvas as a grey reference.
- A hidden layer with Cut enabled is still part of the job.

Move layers up and down to change the cut order. The selected layer is
highlighted on the canvas. The **+** button beside the heading creates an empty
layer and selects it as the move/copy destination.

With objects selected, choose a layer in the **To** row and press **Move** or
**Copy**. Transfers are undoable. Groups move with their members; copied grouped
objects get their own group. Copies are ordinary shapes and follow the
destination layer's cut settings. Raster layers cannot be destinations.

**Delete** in the Selection panel or the `Delete` key removes the selected
objects. An empty selected layer has its own delete control next to the layer
ordering controls.

**Duplicate** (`Ctrl+D`) copies the selection in place and selects the copies.
The **Array** rows keep the original and create a rectangular grid of copies
from column/row counts and `dx`/`dy` spacing. Groups are duplicated as groups.

`Ctrl+C` and `Ctrl+X` place selected objects on the system clipboard as SVG
text. `Ctrl+V` pastes SVG at the canvas pointer, including objects copied from
Inkscape. Pasted paths join an existing vector layer with the same colour; new
layers appear only for new colours.

Clipboard round-trips preserve paths and colours but not editor groups or manual
tabs. Outline text in Inkscape before copying if the receiving application
expects a vector MIME type rather than SVG text.

## Importing

**Open…** replaces the current design with SVG, DXF, PDF, a PDF-compatible AI
file, an image or an `.orleans` project. **Import…** adds geometry to the current
document instead. **Reload** reloads whichever source or project is current.
Dropping an SVG onto the window also loads it.

When geometry is imported or pasted into an existing design, paths whose colour
matches an existing vector layer are appended to that layer. New layers are
created only for colours that are not present yet. Imported layers are appended
at the end of the cut order with unique names, and the action is undoable.

Imported vector geometry is stored as self-contained paths, so hybrid and
DXF-imported projects reload correctly.

### Raster images

PNG, JPEG and BMP open as raster layers with:

- threshold, Floyd–Steinberg, Atkinson or 4×4 Bayer dithering, or grayscale
  power levels;
- line interval, angle, invert, alternate rows;
- position, physical size and DPI;
- **Crop** X/Y/W/H and **Full** to restore the whole image.

The 96 DPI import size is only a starting point; size and DPI can be changed
after import. Cropping is non-destructive: the full image stays visible with a
cyan crop outline while only the cropped region is engraved. Position, size and
rotation stay fixed when the crop changes.

The raster is shown as a translucent image with engraving scanlines over it.
Click an image or select its layer to show handles. Drag inside to move, drag a
corner to resize and drag the top handle to rotate. Hold `Ctrl` while rotating
to snap to 15° increments. Raster layers with Cut disabled show the preview but
not the engraving scan lines. Use a separate vector layer for outlines.

An imported raster always gets its own raster layer, even when a vector layer
has the same colour.

### PDF and AI

PDF files open or import as vector paths, grouped by stroke colour (or fill
colour for fill-only paths). PDF-compatible `.ai` files work too.

Multi-page PDFs ask which page to import; the CLI accepts `--page N`. PDF text
must be converted to outlines. Raster images, clipping paths, gradients,
patterns, transparency groups and annotations are not imported. Imported paths
are not linked to their source file.

### DXF

DXF files open or import as one layer per DXF layer, using the layer's ACI or
true colour. Supported entities are:

`LINE`, `LWPOLYLINE`/`POLYLINE` (including bulges), `CIRCLE`, `ARC`, `ELLIPSE`,
`SPLINE`, `INSERT`/blocks, `TEXT` and `MTEXT`.

`$INSUNITS` sets the millimetre scale. Text is outlined with system fonts; SHX
fonts fall back to a sans-serif face. `HATCH`, dimensions, meshes, binary DXF
and block attributes are skipped. Imported vector layers are self-contained and
do not track later source edits.

## Fill and tabs

Choose **Line** or **Fill** for each layer.

Fill mode adds **Fill spacing**, **Angle**, **Overlap**, **Alternate**, **Cross**
and **Outline**. It hatches closed contours and previews the fill:

- **Cross** adds a second pass at 90°.
- **Offset** fills inward with concentric closed loops instead of scan lines.
- **Outline** traces the closed contours after filling them.
- Open contours are ignored by fill mode.
- Fill outlines do not use holding tabs.

Line mode adds **Tabs**: count and width of uncut holding bridges per contour,
with an optional **Holes** setting for inner contours. Automatic tabs are
distributed evenly by arc length from the contour seam.

Enable **Manual**, press **Place tabs**, then click any closed contour to put a
tab there. Click a tab marker again to remove it. `Esc` or **Done** leaves tab
mode. The selected layer draws each tab span in cyan for both automatic and
manual tabs.

Manual tabs are stored as fractions of the flattened contour perimeter. A heavy
node edit that reorders or removes contours can detach a tab.

## Transform, position and nodes

The **Selection** panel offers numeric Move, uniform Scale and Rotate operations.
The **Position** row places a chosen reference point (corner, edge or centre) at
exact X/Y coordinates relative to the job origin `(0, 0)`; **Move to** applies it.

Double-click any vector object to enter node editing for that shape. The
**Edit nodes** button in the Selection panel does the same for the selected
shape.

While editing:

- Drag an anchor or control point to reshape the path.
- Double-click a segment to insert an anchor.
- Select an anchor and press `Delete` or **Delete nodes** to remove it.
- `Ctrl`/`Shift`-click selects several anchors. `Delete` removes all selected
  anchors in one undoable operation.
- **Break** opens a closed contour at the selected anchor or splits an open path
  there.
- **Join** connects two selected endpoint anchors, closing the contour when both
  belong to the same open path.
- **Snap** snaps node drags to the canvas grid.
- **Geometry** snaps node drags to anchors and segments of other visible shapes.
- Click outside the edited shape, double-click outside it, or use **Done** or
  `Esc` to leave node mode.

Breaking a closed contour leaves the same anchor at both open ends. Joining
different contours inserts a straight line when the endpoints differ.

## Groups, alignment and distribution

**Group** makes the selection one object. Clicking any member selects the whole
group, and drags, handles and align/distribute move it as a unit. **Ungroup**
releases its members.

With two or more objects selected, the **Align** row aligns left/centre/right
and top/middle/bottom. With three or more, **Distribute** spaces their centres
evenly horizontally or vertically.

Groups can span layers. Groups contain vector shapes only and cannot be nested;
grouping a selection that touches existing groups merges them. Ungroup before
node-editing a member. Align/distribute treat each group as one object, and
distribution equalises object centres rather than edge gaps.

## Boolean and offset editing

With two or more closed shapes selected, **Union**, **Subtract** and
**Intersect** appear. Subtract removes the other selected shapes from the first.

**Offset** applies a positive inset or negative outset in millimetres. Boolean
and offset results are flattened polylines; source curves are not reconstructed
after the operation.

## Undo and redo

**Undo** (`Ctrl+Z`) reverses the last transform, node edit, raster move, group
change, import, transfer or layer/job setting change. `Ctrl+Shift+Z` (or `Ctrl+Y`)
redoes it.

## Machine panel

The **Machine** panel connects over TCP/Telnet or USB serial and auto-detects
the firmware. It first asks for a GRBL status report; if none arrives it sends
Marlin's `M115` identity query. The panel then shows the live state, position,
feed and `S` value, with protocol-specific control labels.

Choose **TCP/Telnet** and enter an endpoint such as
`tcp://192.168.1.50`, or choose **Serial**, enter or select a port such as
`/dev/ttyUSB0`, and set the baud rate. **Refresh** lists the serial ports
detected by the operating system. Serial access follows the same
`dialout`/`uucp` permission rules as the CLI.

The **Driver** selector offers **Auto**, **GRBL** and **Marlin**. It can only
be changed while disconnected. **Auto** is the normal setting; a forced protocol
skips detection and is a recovery path when a controller hides its status or
identity reply. Reconnects reuse the same choice. A forced connection can still
be used for Stop and Unlock even when status is missing, but normal jogging and
sending need a readable status.

**Machine profiles** save the connection type, address, baud rate, driver
preference and machine setup together. Use **New profile…** or **Edit** to
open the profile editor, and **Apply** while disconnected to restore a profile.
The active profile is marked with `*` and is restored on the next start.

The profile editor stores:

- work area width and height
- max S and dynamic-power preference
- default speed and power for imported geometry and new layers
- framing speed and margin
- whether homing and soft limits are expected
- free-form notes

Profiles are stored in `~/.config/orleans/machines.json` (or
`$XDG_CONFIG_HOME/orleans/machines.json`).

For GRBL it offers **Home ($H)**, **Unlock ($X)** and **Stop** (feed hold + soft
reset). For Marlin it offers **Home (G28)**, **Unlock (M999)** and **Stop**
(`M5` followed by `M112` emergency stop). A Marlin stop leaves the controller
halted; press **Unlock (M999)** before another job, then re-home or re-zero.

Homing asks for confirmation and is enabled only when the controller is idle or
in alarm, with the laser off.

Long status silences (for example while a controller homes) do not drop the
link. If the WiFi bridge closes the socket, the GUI retries and reports
**Reconnecting…** instead of going offline.

On a GRBL connection, the GUI reads `$$` and shows:

- `$32` (laser mode)
- `$30` (max S)
- `$22` (homing)

It warns when `$32` is off or `$30` differs from **Max S**.

The **Settings…** dialog edits laser mode `$32`, max S `$30`, homing `$22` and
the raw `$$` list. **Apply** writes changed settings to EEPROM. Editing is
blocked while cutting or homing.

The bed/travel area is drawn on the canvas and included in **Fit**. A profile
work area takes precedence; otherwise GRBL `$130`/`$131` is used and offset by
`WCO` when reported. Toggle the overlay with **Bed**.

If GRBL does not report `WCO`, the bed rectangle assumes work zero coincides
with machine zero.

Marlin does not expose GRBL `$` settings. The GUI uses a conservative Marlin
profile instead: `Max S` defaults to `255`, homing is enabled, pause is
disabled, and the settings dialog reports that configuration comes from the
firmware. Marlin's work area and framing defaults come from the machine profile;
GRBL-style `$130`/`$131` travel values are not available.

## Preflight

Before **Send**, the confirmation dialog lists preflight errors and warnings.
Errors block sending; warnings are informational. Checks include:

- controller idle and laser off
- laser mode enabled
- design bounds inside the profile or GRBL work area
- serial port selected for a serial connection
- valid layer speed and power values
- profile homing/soft-limit expectations versus the controller settings

**Frame** runs the same bounds and safety checks using the expanded framing
bounds, including the profile margin. An unknown work area is a warning rather
than an error, so a machine can still be used before its profile is configured.

## Jog and work zero

Jog the head with the X/Y pad, an adjustable step and feed. A separate control
cancels an active jog. The centre button moves to work zero with an absolute
move: `$J=G90` on GRBL, `G90 G0` on Marlin.

**Set zero (G92)** is a temporary work origin: a soft reset, Stop or a controller
reconnect after a reset clears it. **Save zero (G54)** stores it with
`G10 L20 P1 X0 Y0`; whether that offset survives a reset depends on the
controller. Marlin only supports the temporary `G92` form, so **Save zero** is
disabled for Marlin. GRBL jogging needs GRBL 1.1 (`$J=`); Marlin jogging uses
relative `G91`/`G0` moves. Generated GRBL jobs select G54; the Marlin dialect
omits it.

The position readout shows `WPos` when the controller reports it, otherwise
`MPos`.

## Framing, sending and history

**Frame** traces the job bounds with the laser off so placement can be checked
before cutting. The profile sets the framing speed and margin; the preflight
checks the expanded bounds against the work area.

**Send to laser** streams the current job with a progress bar, Pause/Resume and a
per-job Stop button. Pause uses feed hold and Resume uses cycle start. Sending is
enabled only when the controller is connected, idle and the laser is off.
`Ctrl+Enter` opens the confirmation and `Enter` confirms it.

GUI jobs default to **M4 dynamic power**. Pause is offered only when the
controller reports `$32=1`, because a feed hold at zero velocity then switches
the laser off. If a job uses M3, Pause is disabled and Stop must be used.
For GRBL, framing can always be paused because it holds `M5`.

The canvas shows design geometry. Kerf compensation is applied to the toolpath,
so it affects exported G-code and statistics rather than the outline drawing.
Sending starts from the machine's current work origin; set that origin before
pressing **Send to laser**.

Completed, stopped and failed cut/framing runs are added to **Job history** with
time, outcome, duration, cut length and line count. The history is capped at 50
jobs and persisted at:

```text
$XDG_DATA_HOME/orleans/job-history.json
```

or `~/.local/share/orleans/job-history.json`. It can be cleared from the panel.

## Keyboard shortcuts

| Shortcut | Action |
|---|---|
| `Ctrl+O` | Open a design |
| `Ctrl+S` | Save project |
| `Ctrl+E` | Export G-code |
| `Ctrl+Z` | Undo |
| `Ctrl+Shift+Z`, `Ctrl+Y` | Redo |
| `Ctrl+D` | Duplicate selection |
| `Ctrl+C`, `Ctrl+X`, `Ctrl+V` | Copy, cut and paste SVG geometry |
| `Ctrl+Enter` | Confirm and send a job |
| `F`, double-click | Fit the design |
| `Delete` | Delete selected objects or anchors |
| `Esc` | Leave tab or node mode |
