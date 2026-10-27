# Notes on this fork

This fork of Ruffle adds a way to jump between the scenes of a movie, without playing
through it. It's for personal use: user experience comes before keeping permissions
minimal. These notes keep what was learned while making it.

## Repositories and branches

- **This repository**, `VlasenkoMykola/Ruffle_fork`, branch `claude/relaxed-gauss-ymscnb`,
  which is also its default branch. It has the core API, the desktop and web menus, the
  Windows build, and `tools/swfinfo.py`.
- **The Android app**, `VlasenkoMykola/ruffle-android-with-sceneselect`, branch `scene-menu`
  (its `main` is upstream's). Its `Cargo.toml` takes the Ruffle crates from this
  repository's branch, and its `Cargo.lock` pins the commit: after pushing core changes
  here, run `cargo update -p ruffle_core` there and commit the lock file. Its own notes
  are in its `FORK_NOTES.md`.

## Builds and releases

- **Windows**: `.github/workflows/build_windows.yml` builds `ruffle_desktop` for Windows on
  every push to the branch (`--release --features jpegxr`, with the C runtime linked
  statically), and replaces the pre-release `latest-windows-build`, whose zip is at
  https://github.com/VlasenkoMykola/Ruffle_fork/releases/download/latest-windows-build/ruffle-windows-x86_64.zip.
  A new push cancels a build in progress.
- **Android**: its `Build` workflow only runs when dispatched (or for `main`), e.g. with the
  GitHub MCP tool `actions_run_trigger` (`run_workflow`, `workflow_id` `build.yml`, `ref`
  `scene-menu`); dispatch `lint.yml` the same way. Once the APKs are built and the emulator
  tests (API 26 and 36) pass, its `Publish` job (`publish_build.yml`, through
  `workflow_call`) replaces the pre-release `latest-build`:
  https://github.com/VlasenkoMykola/ruffle-android-with-sceneselect/releases/tag/latest-build.
  Its notes name the commit it was built from, which is how to tell builds apart. Each APK
  is also its own unzipped artifact of the run, since the user's connection is slow and they
  only need `app-arm64-v8a-release.apk`.
- Workflow artifacts are unreliable to download for the user, hence the releases.
- From the cloud sessions this was made in: pushing tags is refused (only branches can be
  pushed); `dl.google.com`, Termux mirrors and the artifact storage are blocked, so Android
  builds can't be made locally (`cargo check --target x86_64-linux-android` fails at
  `ring`, which needs the NDK's clang). GitHub release assets download fine, e.g. ktlint
  (`https://github.com/pinterest/ktlint/releases/download/1.5.0/ktlint`) to check Kotlin
  formatting locally; `pip install actionlint-py` checks workflows.

## Core API

In `core/src/root_timeline.rs` and `core/src/player.rs`:

- `Player::root_timeline()` returns a `RootTimeline`: the root clip's scenes (from
  `DefineSceneAndFrameLabelData`, which only AS3 movies usually have; Flash merges AS1/AS2
  scenes into one timeline), frame labels, current frame, total and loaded frames, and
  `game_scenes`.
- `Player::goto_and_play_root_frame(frame)` jumps the root timeline (1-based).
- `GameScenes { names, current }`, with `previous()` and `next()`, is the scene list of games
  that keep their own (see below), and `Player::goto_game_scene(index)` switches to one.
- Frame jumps: for movies with no game scene list, scene clips, scenes or frame labels,
  `Player::root_timeline()` makes the frames that the movie's own code jumps its root
  timeline to into `scenes`, named "Frame N" (from frame 1, each up to the next), and sets
  `scenes_are_frame_jumps`, so the frontends' scene menus list them with no changes of
  their own. `frame_jumps` in `root_timeline.rs` reads them from the tags: `gotoAndPlay(n)`
  and `gotoAndStop(n)` (GotoFrame, or GotoFrame2 after a pushed number) in the code of the
  root timeline (functions included) and of the buttons placed on it, and
  `_root.gotoAndPlay(n)`, `_level0...` or `tellTarget("_root")` in any code. For old games
  that keep their parts on one long timeline, like Tsunade's Debt (1524 frames, 36 jumps
  on the main timeline, no labels). Reading all of Ruffle's 4931 test SWFs this way takes
  3.4 s in all. `code_blocks` lists the code it reads, with whether each block's own target
  is the root.
- Frame variables: games like Universal Soldier pick the version of a character a clip
  shows with a variable set earlier (`gotoAndStop(_root.char)`), which a menu jump skips,
  leaving the clip playing through all its versions. Before each jump,
  `Player::goto_game_scene` and `Player::goto_and_play_root_frame` call
  `fill_frame_variables`, which gives each variable read in the frame of a
  GotoFrame2 or `gotoAndStop`/`gotoAndPlay` call that has no value the first constant the
  code gives a variable of that name (by `SetVariable`, `SetMember` or a timeline's
  `var`), or else 1. `frame_variables` finds them, following the stack like `scan_jumps`
  (with registers, and the ones `DefineFunction2` preloads): `_root.x`, `/:x`, `_level0.x`,
  `this.x` and a bare `x` in the root's code are the root's; `_global.x` and a bare `x` in
  a clip's code (from SWF 6, as a clip without it reads `_global`'s) are `_global`'s;
  `_parent.x` in a clip's code is taken for the root's. Names starting with `_` are left
  out, as properties, and so are the ones `_global` has, getters included (`has_property`),
  like `NaN`, which a bare `NaN` in the root's code would otherwise have set on the root. Versions picked with conditions, like Gotham City's girls, which
  show the person next to them with `if (_root.person1nearGirl1 == true) gotoAndStop(1)`
  and so on (set to false on the screen where they're placed, and true by its buttons):
  in a clip's own code (not in its functions), an `If` on `variable == constant` (or a
  bare variable, for true) that a goto to a fixed frame follows before the `If`'s target
  is a condition of `Versions`; a clip with more than one, that doesn't stop by itself
  (`stop()`, or a fixed goto outside the conditions), picks versions, and if none of its
  conditions holds when the menu jumps, the first one is made to hold (each girl then
  shows the first person). Both are read from every loaded AVM1 movie
  (`Library::known_movies`), as games like Gotham City keep their scenes in parts they
  load, whose code uses `_root`'s variables.
- Tests: `cargo test -p ruffle_core --lib root_timeline`. They build their game movies with
  the `swf` crate, in the variable layouts below, and with exported clips for the version
  without a list.

### How games with their own scene list are handled

A movie counts as such a game when its root timeline has a `setScreen` function and an
array in `Qvar`, `scenelist`, `screens` or `screens_list` (each looked up on the root, then
in `_global`, like the other variables below), or, without those, exports movie clips named `screen1`,
`screen2`... or `screen_1`, `screen_2`... (the list is then `mainmenu`, the numbered ones in
order, and the other `screen_*` ones, like `screen_end`). A list that names none of the
movie clips the root movie exports gives way to those clips when there are some: One Piece
of Luck's launcher leaves `_global.screens` (`screen_0`...) for the part it loads over
itself, which exports `mainmenu`, `screen1`... and has no list, so the menu called
`setScreen` with names the part doesn't have, and showed a blank screen. One Piece Nami
has 18 numbers in `Qvar`, which aren't scenes, and exports `main_menu`, `screen_1`...
`screen_6`; it gets its clips the same way. Big Queen 1's parts export clips with the
names in its launcher's list, which is kept. The scene being shown is the one named in
`screen_name`, or else numbered (1-based) in `Qpos`, `pos`, `Qmod` or `gn`, or else, for exported
clips, the one the root's child `screen` is an instance of. To switch, core does what the
games' own controls do: stop all sounds, show the mouse cursor, and call the root's
`setScreen` with:
- the difference between the scene's number and `Qmod`, when the root has `Qmod` (Hitomi's
  `setScreen` moves on by that many scenes);
- else the 1-based number of the scene, when the root has `Qfrm`;
- else its name.
(An earlier approach set `Qscr`, which only works in the versions that check it every
frame.)

## The games' engine

Most of the games tested are made by one developer, with one engine in several versions.
What's known about each, from `tools/swfinfo.py` dumps:

0. **Version without a list** (Avatar The Last Airbender, Baka 1, Wheel of Wonder, Universal
   Soldier, Tripping the Rift Six Satisfaction). One SWF, whose scenes are movie clips
   exported as `mainmenu` (or `main_menu`), `screen1`... (plus `screen_end`, `screen_failed`,
   `screen_demo`, `screen_1_1`...), or as
   `screen_0`, `screen_1`... in Universal Soldier (whose `Qscr` n is `"screen_" + (n - 1)`)
   and Tripping the Rift (which starts at `setScreen("screen_5")`).
   `setScreen(clip_name)` removes `screen` and attaches the clip as `screen` on the root;
   `gotoScreen(clip_name)` plays `trans_screen` first. Baka also sets `screen_name`, sends
   `fscommand("bar", n + 1)` for `screenN` (1 for the main menu) and `fscommand("mnf", 10)`,
   and checks `Qscr` every frame (1 is the main menu, n is `screen(n - 1)`), stopping sounds
   and showing the mouse first. Its `parts/red.bar` is only its progress bar.

1. **`scenelist` version** (Olympic Contest, April Story). A small launcher SWF that loads
   its parts. Variables on the root: `scenelist` (array of names like `screen_0`),
   `scenepart` (the part each scene is in), `pos` (current, 1-based), `Qscr` (requested).
   `setScreen(name)` shows a scene, after loading its part with `LoadGame(part)` if needed,
   which loads `filepath + filename + part + fileext` (`"parts/" + "obt" + n + ""`, so the
   part files have no extension) into the `game` clip with a `MovieClipLoader`. A
   `gameHandler` checks `Qscr` every frame, and the keys Page Up (33) and Page Down (34)
   (previous and next scene), and M or numpad minus (stop sounds). It only starts when
   `_url.substring(2, 4) == "le"`, which is true for `file://` URLs; elsewhere it opens the
   developer's site. Sends `fscommand("bct", count)` and `fscommand("bar", pos)`. Olympic
   Contest is protected: junk code and decoy constant pools behind branches always taken.
2. **`Qvar`/`Qpos` version** (Nice Christmas 2, Titans 2, Legend of Korra). Also a launcher
   with parts (`parts/nc2`, `parts/titans2`, `parts/loksf`), loaded into the `game` clip
   (`loader.loadClip(_root.game_path, game)`), plus per-screen extra parts through a
   `LoadingManager` class and a `socket()` function (`"parts/nc2" + screen_name +
   parts_progress`). Root variables: `Qvar` (array literal of names: `mainmenu`,
   `screen1`...), `Qmax` (count), `Qpos` (current), `Qscr` (requested, checked by the root's
   `onEnterFrame`, with the same keys). `setScreen(name)`, and `gotoScreen(name)`, which
   plays the `trans_screen` transition first.
3. **`Qvar`/`Qfrm` version** (Mortal Combat parody, Catwoman, Avatar, Hitomi).
   One SWF: the scenes are exported movie clips (`screen1`... or hashed names like
   `e14865a3` in Avatar), attached into the `game` clip. `Qstr` is a comma-separated string
   of the names, `Qvar = Qstr.split(",")`, `Qfrm` the count. No current number:
   `screen_name` holds the current name. `Qscr` is only the first scene, and nothing checks
   it later; there are no Page Up/Down keys. `setScreen(x)` attaches `Qvar[x - 1]`; in Mortal
   Combat and Catwoman it first calls `getposition(x)`, which takes a number or a name, while
   Avatar has no `getposition`, so it needs the number, and attaches to the root. Hitomi's
   `setScreen(x)` is relative: `Qmod = Qmod + int(x)`, back to 1 when that's past `Qfrm` or
   below 1, then attaches `Qvar[Qmod - 1]` into `game` (its scene clips call
   `gotoScreen(1)` to go on); it builds `Qvar` itself when called with `undefined`, which its
   first frame does (with `Qmod` 0). Passing it the scene's number made most choices
   overshoot to the main menu. Hitomi's first frame also carries MD5, RSA and `XMLSocket`
   code for `secured_key.php`, after the `End`, so it never runs. `gotoScreen(x)` attaches
   `trans_screen`, which calls
   `_parent.setScreen(next_screen)` at its frame 10. Sends `fscommand("mnf"/"mnf1", Qfrm)`.
   The scene clips of Catwoman and Avatar have their own frame labels for the stages within a
   scene (`act0`...`act3`, `trans1`...`trans3`, `result`, or `hold1`...`hold5`): a possible
   next feature is listing the labels of the scene being shown (`game.screen`) to jump
   between them.

Unknown to `--classify`, but of the same developer (samples sent so far):

- **Big Queen 2**: a launcher of the `Qvar` version (12 scenes, `socket`, `parts/mq2`); it was
  supported all along, and the menu works in it (tested).
- **Big Queen 1** and **Evil Sorceress** (the menu doesn't work in them yet): launchers that load their first part over
  themselves (`loadMovie("parts/btmq0", this)`, and into `_level0` from `parts/esrm`), so
  the game's code is in the parts. Their arrays are like the `scenelist` version's, with
  other names: `screens` (and `fscommand("bct", 11)`), and `screens_list` with the part of
  each scene in `screens_arr`. Each part (SWF 15, AS2) has its own `setScreen` and
  `gotoScreen`, exports the clips of its scenes (`screen_0`...`screen_3` in `btmq0`,
  `screen_4`...`screen_7` in `btmq1`, `screen_8`...`screen_10` in `btmq2`; `screen_0`,
  `screen_1`, `screen_5` in `esrm1`, `screen_2`...`screen_4` in `esrm2`, `screen_6`...
  `screen_10` in `esrm3`, as `screens_arr` says), and loads the other parts over the root
  in turn (`loadMovie("parts/btmq1", this)`; `esrm` ones into `_level0`). The launchers keep
  their variables in `_global`, so they survive the parts replacing the root: Big Queen 1
  sets `_global.screens` and `_global.Qpos = 1`; Evil Sorceress sets `_global.gn = 1`,
  `_global.screens_list`, `_global.screens_arr` and `_global.qfile = "parts/esrm"`, then
  loads `qfile + "1"` into `_level0`. The parts' `setScreen(name)` handles a scene in
  another part itself: Big Queen 1's sets `_root.Qscr` (and `Qset`) for its `onEnterFrame`,
  which stops sounds, shows the mouse, sets `_global.Qpos`, and attaches the clip or loads
  `btmq1` (scenes 5 to 8) or `btmq2` (9 to 11) over the root; Evil Sorceress's sets
  `_global.gn`, then attaches the clip when `screens_arr` puts it in the part loaded
  (`_root.screens_cur`), or else loads `qfile + screens_arr[i]` into `_level0`, whose
  first frames set `Qscr = _global.gn` for its `onEnterFrame` to show. Both have the Page
  Up/Down and M keys. Core reads their lists and current numbers from `_global` and calls
  `setScreen` with the name; `--classify` counts their launchers as supported when the
  parts they name have `setScreen`.
- **Avatar The Last Airbender** and **Baka 1**: the version without a list (0 above), which
  the menu supports since it lists the exported scene clips.

- **The Iron Giant 3** and **Super Family 2**: launchers like Olympic Contest's (`LoadGame`,
  `gameLoader`, `qfile` = `parts/irong3`, `setScreen` and `gotoScreen` in the launcher),
  with Evil Sorceress's list names `screens_list` and `screens_arr`. The menu works in them
  (tested); `--classify` counts them since `6b2dbd238`, which gave it core's list variables
  (`SCENE_LISTS`).

Other games:

- **Lara Kroft Commando Recon**, **Tifa's Happy Xmas** and **Kasumi Training**: the version
  without a list, protected: their main timeline's functions have garbled names like `♦♣`,
  while `setScreen` is still there by name when they run (the menu worked in all three).
  `--classify` takes `setScreen` from the names in the code too, since `467e51a2a`. Tifa
  and Kasumi name their main menu `main_menu`, which core lists first since then; their
  `end` clips aren't listed.
- **Matsumoto vs. Tsunade**: a launcher that only loads `parts/mvt0_1_2` over itself; the
  part has the scene code (the menu works). `--classify` judges launchers by the scene code
  of the parts they name (up to 6 read) since this was found.
- **Metropolis Files** 1 to 4: another version (4 KB protected launchers). The launcher sets
  `papka = "parts/"`, `files = ["mmtxf1", "mmtxf2"]` and `start_file = {name: "mmtxf2",
  screen: "screen_0"}`, and `GameFile(name)` loads every part at once with `loadClip` into
  its own clip (`_lockroot`, hidden but for the start part). The parts talk to the launcher
  through a fake `XMLSocket` (`XMLSocketWrapper`; `LoadVarsWrapper` answers their
  `LoadVars`): a message of four comma-separated fields reports the part's scenes in the
  last one, `;`-separated, which `srv_socket_recv` keeps in the part's `screens`; any other
  message is a scene to go to. `srv_send_screen(name)` finds the part whose `screens` has
  it, swaps it in as the visible `cur_file`, and calls `cur_file.clip.gotoScreen(name)`.
  `Array_indexOf` only finds anything when `_url.substring(2, 4) == "le"` (a `file://`
  URL). Core lists the scenes the parts reported (sorted by number when all have one),
  takes the current one from `cur_file.clip.screen_name`, and switches with
  `srv_send_screen(name)`, which works in all four (tested on Android `17eb663`).
- **boundary expansion lab** (AS3, `loadBytes`): doesn't run in Ruffle at all, so not worth
  supporting.

- **Digital Adventure** (another developer, AS3): its scenes look like SWFs embedded in the
  main one (`Main_s0_class`...`Main_s8_class`, from `s0.swf`...`s8.swf`), loaded by a `Main`
  class. Not supported yet; a `--dump` of it shows its classes and methods.

User testing so far: scene switching works in Nice Christmas 2, Olympic Contest, Titans 2,
Mortal Combat, Avatar, Hitomi, Big Queen 1 and 2, Evil Sorceress, Avatar The Last Airbender
and Baka 1 (Android `030412c`, Windows `e665692b3`). Mortal Combat first did nothing, on the
build that set `Qscr`, and many of Hitomi's choices led to the main menu until core gave
its relative `setScreen` the difference. Avatar's and Hitomi's menus show the games' own
hashed names, like `540e8d62`. Wheel of Wonder (exported `screen1` and `screen2`, no
`mainmenu`), Vee Hidden in the Steam and Teenage Mutant Turtles (like Evil Sorceress), and
The Iron Giant 2 (like Big Queen 1, with its `screens` out of numeric order and `qjumps`, a
0-based part number per scene, for `parts/tig20`...`tig22`) work too; the user's script
copy had called them unknown. So do Universal Soldier and Tripping the Rift Six
Satisfaction (`screen_N` clips, Android `62e9ae1`), and Tsunade's Debt with frame jumps
(Android `8043e57`), which the user still counts apart from supported games. On the user's collection,
`--classify` found 532 of 693 games supported, 10 with frame labels only and 151 unknown,
and 576 supported and 107 unknown after the versions above were added (`e665692b3`). After
the user removed duplicate files: 436 games, 343 supported, 7 frame labels only, 86 unknown
(with `screen_N` clips, `dbde61b34`), and then, with frame jumps (`8c99a6b68`), 343
supported, 7 frame labels only, 13 only frame jumps supported and 73 unknown; with The Iron
Giant 3's version (`6b2dbd238`), 350 supported and 66 unknown, of which the user counts only
10 to 20 as real games, the rest being SWFs bundled in HTML games. Counting games only
(`--onefolderdeep`, `122dc6237`): 353 games, 325 supported, 7 frame labels only, 7 only
frame jumps supported, 14 unknown; with `setScreen` found by name in protected games
(`467e51a2a`), 332 supported and 7 unknown; with launchers judged by their parts and
Metropolis Files' version (`92c11d307`), 338 supported, 7 frame labels only, 7 only frame
jumps supported and 1 unknown (boundary expansion lab, which doesn't run in Ruffle): every
game of the collection that runs in Ruffle is supported.

After menu jumps (Android `a2dfe36`, Ruffle `f12e7bad7`): Universal Soldier's character no
longer plays through its versions (frame variables), nor do Gotham City's girls (version
conditions, read from its part `gcs-game`), and One Piece of Luck's and One Piece Nami's
scene menus, which left a blank screen, work in full (their exported clips over a list
naming none of them).

## Frontends

- **Desktop** (`desktop/src/gui/menu_bar.rs`): a Scenes menu with the game's scenes or the
  timeline's scenes, Previous and Next Scene, a Frame Labels submenu, and a frame field with
  Go. Texts in `desktop/assets/texts/en-US/main_menu.ftl`. The menu stays open on clicks
  inside it (`PopupCloseBehavior::CloseOnClickOutside`), so the frame field can be used.
- **Web** (`web/src/lib.rs`, `web/packages/core/src/internal/player/inner.tsx`): the context
  menu lists the timeline's scenes. It doesn't have the game scene lists yet.
- **Android**: see the app's `FORK_NOTES.md`.

## tools/swfinfo.py

A script for the user to run on their own games (Python 3, standard library only), whose
output they paste back, since the games themselves aren't shared:

- `python swfinfo.py <files or folders>`: a summary of each SWF: version, frames, scenes,
  labels, stops, gotos, loads, main timeline functions, file names, fscommands and arrays
  in the code, movie clips with labels or long timelines, exported symbols, AS3 classes and
  strings, which other given files it names, and embedded SWFs, described after it.
- `--dump`: also the tags, an outline of the AS3 classes (fields, methods, and what each
  method uses), and a disassembly of all AS1/AS2 code. `--dump-main`: only the main
  timeline's code, for big games.
- `--classify <folders>`: one line per game on how it handles scenes (supported, frame
  labels only, only frame jumps supported, unknown, unreadable), and a tally. "Only frame
  jumps supported" is for games the menu can only offer the frame jumps above for, as
  read by the script's `frame_jumps` the same way, which the user doesn't count as
  supported; `--scenes` lists their frames. SWFs in a subfolder that a game names
  (like `parts/`), or named by another SWF next to them, count as its parts.
- `--onefolderdeep` (with `--classify`, `--unsupported`, `--unsupported-names`): only the
  SWFs in the given folders and their direct subfolders count as games, leaving out SWFs
  bundled deeper in HTML games; launchers' parts are still read from their `parts` folders.
- `--unsupported <folders>`: the same, listing only the games that aren't supported (the
  tally still counts all of them), to find the next samples. `--unsupported-names
  <folders>`: only the paths of those games, one per line.
- Names with `*` or `?` are expanded by the script, since the Windows command line doesn't.
- `--scenes <files>`: for games with their own scene list, each entry with the clip the
  movie exports under that name (frames, labels, which engine functions and variables its
  code and the code inside it use), or that it has none; then the main timeline's code.
- `--menu-changes` (with `--classify` or `--unsupported`): then lists the games that a jump
  from the scene menu does more to than show the scene, by core's rules, ported to the
  script (`frame_picks`, `clips_over_list`, `menu_changes`, reading the parts each game
  names with `game_parts`): the variables it gives values, the version clips it shows the
  first version of, and the exported clips it lists instead of a scene list naming none,
  for the user to re-test. `statements` tracks, for this, the variables' homes, `==`
  conditions, fixed gotos and tellTarget scopes, like core's `scan_variables`.
- `--variables <files>`: the code that picks a timeline's frame with a variable (or, in a
  clip, with a condition), like a character's version picked earlier in the game, then
  where the game sets each variable that code reads (with the frame a button sits on,
  and the function an assignment is in). Then the clips that other code moves between
  frames (`gotoAndStop`, `nextFrame`, `stop`... called on them, or in `tellTarget`) or shows
  and hides (`_visible`), with the clips placed under that name and whether each stops
  itself, for games like Gotham City, whose clips no variable picks. Scenes opened from the
  scene menu skip that code, leaving such clips playing through all their versions. Its `statements`
  rebuilds statements from the bytecode's stack, as far as reading without running tells.

It reads AVM1 code the way it runs (following jumps, branches and functions), so junk after
jumps doesn't hide code, and it ignores garbled constant pools as decoys. Changes are best
checked against Ruffle's test SWFs (`tests/tests/swfs/avm1` and `avm2`), which it should all
read without errors.

## tools/push_games.py

Copies the user's games to their Android device with adb (Python 3, standard library only):
`python push_games.py [--dest /sdcard/SWF_Games] [--serial X] [--adb path] [--dry-run]
[folder]`. Each direct subfolder of the folder (the current one by default) with a `.swf`
file in it is a game, copied whole to the device's folder of the same name, unless a folder
of that name is already there (ignoring case). Games are pushed as `<name>.partial` and
renamed when complete, so a cut-off copy is redone on the next run. It finds adb on the
PATH or in Android Studio's `%LOCALAPPDATA%\Android\Sdk\platform-tools`, quotes names
for the device's shell (games have names like `Tsunade's Debt`), and was tested against a
stand-in for adb, as no device was connected to the session.

`tools/games_size.py [folder]` lists the size of each of those games (folder and all),
largest first, and their total, to see the space they need; `push_games.py --dry-run` gives
the size of only the ones not on the device yet.

`tools/scene-test-swfs` has a small AVM1 assembler (`asm.py`) and `mc_like.py`, which writes
a movie with the scene code of Mortal Combat's version of the engine, used to check that
version's scene switching in core.

## Open questions for the user

- Merging `scene-menu` into the Android app's `main` (which would make each push publish).
- Giving the Android fork its own app ID, so it installs next to the official app.
- Removing Dependabot from this repository, and closing its pull requests.
- Whether to list the stages within a scene (the labels of the scene's clip).
