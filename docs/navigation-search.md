# Navigation, search and organization

Arrow keys and Tab keep the active cell visible horizontally and vertically,
including wide tables and resized columns. Shift extends the selection.

## Find in a table

Ctrl+F (Command+F on macOS) opens a search bar in the data grid. Type to select
the first matching cell. Enter/Next moves forward, Shift+Enter/Previous moves
backward, and Escape closes the bar. Search wraps and follows the current
sort/filter order, including pending cell values.

This searches the loaded rows. Use **Find in Schema…** to search the database.

## Find in Schema

Right-click empty space in Objects and choose **Find in Schema…**. Select the
database, optionally specify a schema (blank means all), and enter a string or
value. Choose substring or exact-value matching and case sensitivity.

Search visits all accessible user tables in that scope, including numeric and
date columns converted to text. Binary/spatial columns are excluded. Text
conversion and case folding follow the database's rules. Nulls do not match.
Views are excluded to avoid repeatedly scanning the same data.

Each matching table offers a read-only preview of up to 100 matching rows;
`100+` means at least 100 were found, rather than an exact count. Select a table
to inspect the preview, or choose **Open table** to open its data editor.
Per-table errors are listed. Cancellation and timeouts show how many tables
were attempted; an interrupted search is never reported as complete.

Search runs in the background on a separate session and only issues SELECTs.
It does not use the editor's transaction. Setup has a 30-second deadline and
each table query a 15-second deadline. Cancellation/timeout closes that search
session after attempting native cancellation.

## Object groups and colors

Groups appear as folders alongside objects, without a separate Groups pane.
Right-click empty space, choose **New group…**, enter its name and save.
Double-click a folder (or select it and press Enter) to browse its contents;
**All objects** returns to the parent view.

Ctrl/Command-click toggles individual objects, Shift-click or Shift+arrows extends
a range, and Ctrl/Command+A selects all visible objects. Drag a rectangle from
empty space to select several objects with the mouse. Right-clicking an already
selected table keeps the full selection for **Add to group…** and **Color**.

Drag selected tables onto a folder to move the entire selection into it. Inside
a folder, drop onto **All objects**, or choose **Add to group… → Ungrouped**, to
move tables out. A folder's context menu offers rename and delete. Deleting a
folder removes only its grouping and retains object colors.

Groups and colors are saved in application preferences per connection/database,
with separate entries for objects of the same name in different schemas. They
do not create schemas, rename tables, or write organizational metadata to the
database. They apply to both the object list and grid.

The selected list/grid layout is saved with application settings. **Save on row
change** is also remembered and used by existing and newly opened table editors;
it remains disabled by default.

Right-click one writable table and choose **Truncate…** to open the existing
confirmation and SQL preview. Truncate is disabled for views, read-only
connections and multiple selections; choosing the menu item does not execute SQL.

## Closing tabs

Editor and result tab menus offer Close, Close others, Close to the left,
Close to the right, and Close all. Bulk closing uses the existing pending-edit
confirmation, so cancelling leaves the selected tabs open.
