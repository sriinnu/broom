# Changelog

## Unreleased (0.7.0)

**New**
- **Clone-aware totals.** Folder totals count blocks that APFS clones share once, so a whole-disk scan adds up to what the disk holds instead of reading above its size. A clone still shows its full size, with a *clone* tag. A folder holding clones shows *+X shared*, and its slice in the chart is hatched by that much.
- **Merge as Clones shows its saving.** The folder totals drop by what the banner says was freed.
- **Where is my space?** gets a *Shared by clones* line, inside *Files Broom read* (not added again).

**Known limit:** a clone that was edited after copying still shares some blocks; those are counted once per file.

## 0.6.0 (2026-11-06)

**New**
- **Empty Trash** (⇧⌘⌫). Says what's in the Trash first, then shows how much space really came back.
- **Merge as Clones.** Duplicate files become APFS clones: every file stays, the extra space comes back.
- **Where is my space?** (View menu) Your disk as lines that add up, each marked measured, computed, estimated or unknown. Snapshots are named, and local Time Machine ones can be deleted.
- **Never Offer This** (right-click, or Settings › Protected). Broom, and agents using the CLI, will never move it or anything holding it to the Trash.
- **Developer caches, per project.** Build outputs (`target`, `.next`, `Pods`, …) and Python environments. A build folder only counts with its tool's file beside it, and never if git tracks it. Projects changed this week stay unticked.
- **"Use its tool"** for Docker, Go modules, Rust toolchains, Ollama and simulators: no checkbox, just the right command.
- A livelier trash drop. Reduce Motion fades instead.

**Fixed**
- Menu commands act on the main window when Settings is in front.
- Big cleanup plans no longer lag.
- The CLI only reports space from what actually moved.

**Safety**
- Agents (the CLI) can only move things to the Trash. A test fails the build if that ever changes.

**Known limit:** shared storage (clones, hard links) is counted once per file in folder totals, so a whole-disk scan can read a little high.

## 0.5.1 (2026-11-06)
- Opens instantly from the last scan, then catches up with only what changed.
- Proper macOS menus, Settings in its own window, new toolbar icons.
- Hide the findings column (⌥⌘0). A little delight after a cleanup.

## 0.5.0 (2026-11-04)
- Back and forward through folders, multi-select, date created and modified columns.
- Readable names, working control-click, sidebar toggle.

## 0.4.0 (2026-10-26)
- Resizable panes, sorting, copy path.
- What grew since the last scan, duplicate files, Put Back.

## 0.3.1 (2026-10-24)
- Fixed the release build. Install with Homebrew; releases are notarized.

## 0.3.0 (2026-10-24)
- Five looks: Classic, Darkroom, Ledger, Console and Bloom, in light and dark.

## 0.2.0 (2026-10-23)
- First release as Broom: menu-bar search and clean, the redesigned window, the investigator.
- CLI: `scan`, `find`, `clean`, `trash`.
- Follows the Trash live, one Broom at a time, says how much is purgeable.
