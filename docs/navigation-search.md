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

The Groups pane provides local folders for organizing tables and views. Enter
a name and choose **New group**. Right-click an object and use **Move to group**
or **Color**. Select a group to browse its contents; **All objects** and
**Ungrouped** provide the other views. Rename/delete operates on the selected
group. Deleting a group returns its objects to Ungrouped and retains colors.

Groups and colors are saved in application preferences per connection/database,
with separate entries for objects of the same name in different schemas. They
do not create schemas, rename tables, or write organizational metadata to the
database. They apply to both the object list and grid.

## Closing tabs

Editor and result tab menus offer Close, Close others, Close to the left,
Close to the right, and Close all. Bulk closing uses the existing pending-edit
confirmation, so cancelling leaves the selected tabs open.
