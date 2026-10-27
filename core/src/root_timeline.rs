//! Snapshot of the root movie clip's timeline.
//!
//! Frontends use this to offer scene and frame label navigation, letting the
//! user jump anywhere on the root timeline without playing through the movie.

use crate::avm1::{Activation, Avm1, Object as Avm1Object, Value};
use crate::character::Character;
use crate::context::UpdateContext;
use crate::display_object::{DisplayObject, MovieClip, TDisplayObject, TDisplayObjectContainer};
use crate::prelude::CharacterId;
use crate::string::{AvmString, WStr};
use crate::tag_utils::SwfMovieData;
use std::collections::{HashMap, HashSet};
use swf::avm1::types::{Action, FunctionFlags, Value as Avm1Value};
use swf::{SwfStr, Tag, TagCode};

/// The variables in which games made with a certain engine keep their own list of
/// scenes, in each version of the engine. Like the other variables of the engine, they're
/// variables of the root timeline, or of `_global` in the versions whose launcher loads
/// its parts over itself, like Big Queen 1's (`screens`) and Evil Sorceress's
/// (`screens_list`).
///
/// Those games are a launcher movie that loads its scenes from other files, or keep
/// each scene in a movie clip, so the root timeline has no scenes of its own. They
/// switch scenes by calling the [`SHOW_SCENE`] function of the root timeline with the
/// name of a scene from the list, or its number in the versions that count them.
const SCENE_LISTS: [&str; 4] = ["Qvar", "scenelist", "screens", "screens_list"];

/// The function of the root timeline that switches to the scene with the given name.
const SHOW_SCENE: &str = "setScreen";

/// The variable of the root timeline that holds the name of the scene being shown.
const CURRENT_SCENE_NAME: &str = "screen_name";

/// The variables that hold the 1-based number of the scene being shown, in the versions
/// of the engine that have one.
const CURRENT_SCENE_NUMBERS: [&str; 4] = ["Qpos", "pos", CURRENT_SCENE_TO_MOVE_FROM, "gn"];

/// The variable of the root timeline that holds the 1-based number of the scene being
/// shown, in the version of the engine whose [`SHOW_SCENE`] moves on from that scene by
/// the number of scenes it's given, like Hitomi's.
const CURRENT_SCENE_TO_MOVE_FROM: &str = "Qmod";

/// The variable of the root timeline that holds the number of scenes, in the version of
/// the engine whose [`SHOW_SCENE`] takes the 1-based number of a scene. The versions
/// without it take its name.
const SCENE_COUNT: &str = "Qfrm";

/// The most scenes read from a game's scene list.
const MAX_GAME_SCENES: i32 = 1000;

/// The export names of the movie clip of the main menu, in the version of the engine that
/// has no scene list: its [`SHOW_SCENE`] attaches the exported movie clip with the name
/// it's given, and the names of the others start with [`SCENE_CLIP_PREFIX`]. Some games,
/// like Tifa's Happy Xmas, call it `main_menu`.
const MAIN_MENU_CLIPS: [&str; 2] = ["mainmenu", "main_menu"];

/// The start of the export names of the movie clips of scenes, in the version of the
/// engine that has no scene list: `screen1`, `screen2`... (or `screen_1`, `screen_2`...),
/// and others like `screen_end`.
const SCENE_CLIP_PREFIX: &str = "screen";

/// The function of the root timeline that shows the scene with the given name, in the
/// version of the engine whose launcher loads all its parts at once, like Metropolis
/// Files': it shows the part that has the scene, and calls the part's `gotoScreen`.
const SHOW_SCENE_IN_PARTS: &str = "srv_send_screen";

/// The variable of the root timeline that holds the parts, in that version: an array of
/// objects, each with the names of the part's scenes in `screens`, which the part reports
/// once it has loaded.
const SCENE_PARTS: &str = "files";

/// The variable of the root timeline that holds the part being shown, in that version,
/// whose movie clip is in `clip`.
const CURRENT_SCENE_PART: &str = "cur_file";

/// The instance name of the movie clip showing the scene, in the version of the engine
/// that has no scene list.
const SCENE_CLIP_INSTANCE: &str = "screen";

/// A scene on the root timeline.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct TimelineScene {
    pub name: String,

    /// The 1-based frame number this scene starts at.
    pub start: u16,

    /// The number of frames in this scene.
    pub length: u16,
}

impl TimelineScene {
    /// The 1-based frame number of the last frame of this scene.
    pub fn end(&self) -> u16 {
        self.start
            .saturating_add(self.length)
            .saturating_sub(1)
            .max(self.start)
    }
}

/// A frame label on the root timeline.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct TimelineLabel {
    pub name: String,

    /// The 1-based frame number this label is placed on.
    pub frame: u16,
}

/// The scenes, frame labels and playhead position of the root movie clip.
#[derive(Clone, Debug)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize),
    serde(rename_all = "camelCase")
)]
pub struct RootTimeline {
    /// Scenes in playback order.
    ///
    /// Scene names only survive publishing in movies that contain a
    /// `DefineSceneAndFrameLabelData` tag, which Flash writes for AS3 movies.
    /// Flash merges the scenes of AS1/AS2 movies into one timeline, so for
    /// those this is usually empty and `labels` is the best way to navigate.
    ///
    /// For movies with no other way to navigate, these are the frames that the
    /// movie's own code jumps its root timeline to, named "Frame N": see
    /// `scenes_are_frame_jumps`.
    pub scenes: Vec<TimelineScene>,

    /// Whether `scenes` are the frames the movie's own code jumps the root timeline
    /// to, as it has no scenes, frame labels or scene list of its own, like old games
    /// that keep each of their parts on frames of one long timeline.
    pub scenes_are_frame_jumps: bool,

    /// Frame labels sorted by frame number.
    pub labels: Vec<TimelineLabel>,

    /// The 1-based frame number of the playhead.
    pub current_frame: u16,

    /// The number of frames declared in the SWF header.
    pub total_frames: u16,

    /// The number of frames loaded so far; frames past this can't be jumped to yet.
    pub frames_loaded: u16,

    /// The game's own list of scenes, for games that keep one in variables.
    pub game_scenes: Option<GameScenes>,
}

/// The scenes of a game that keeps its own list of them in a variable of its root
/// timeline, which [`crate::Player::goto_game_scene`] switches between.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct GameScenes {
    /// Scene names, in the game's order. Names that aren't strings are empty.
    pub names: Vec<String>,

    /// The index into `names` of the scene being shown, if the game says.
    pub current: Option<usize>,
}

impl GameScenes {
    /// Reads the scene list of the game whose root timeline is `root`, if it has one.
    pub(crate) fn read<'gc>(
        context: &mut UpdateContext<'gc>,
        root: DisplayObject<'gc>,
    ) -> Option<Self> {
        let object = root.object1()?;
        Avm1::run_with_stack_frame_for_display_object(root, context, |activation| {
            let list = SceneList::of(root, object, activation)?;
            let names: Vec<_> = (0..list.len())
                .map(|index| match list.scene(index, activation) {
                    Value::String(name) => name.to_string(),
                    _ => String::new(),
                })
                .collect();

            let current = match variable(object, CURRENT_SCENE_NAME, activation) {
                Value::String(name) => {
                    let name = name.to_string();
                    names.iter().position(|scene| *scene == name)
                }
                _ => None,
            }
            .or_else(|| {
                CURRENT_SCENE_NUMBERS.iter().find_map(|name| {
                    match variable(object, name, activation) {
                        Value::Number(number) if number >= 1.0 && number <= names.len() as f64 => {
                            Some(number as usize - 1)
                        }
                        _ => None,
                    }
                })
            })
            .or_else(|| match &list {
                // The clip showing the scene is one of the scenes' clips.
                SceneList::Clips(clips) => {
                    let shown = root
                        .as_container()?
                        .child_by_name(WStr::from_units(SCENE_CLIP_INSTANCE.as_bytes()), false)?;
                    clips.iter().position(|(_, id)| *id == shown.id())
                }
                // The part being shown keeps the name of its scene, like the other versions.
                SceneList::Parts(_) => {
                    let Value::Object(part) = stored(object, CURRENT_SCENE_PART, activation) else {
                        return None;
                    };
                    let Value::Object(clip) = stored(part, "clip", activation) else {
                        return None;
                    };
                    let Value::String(name) = stored(clip, CURRENT_SCENE_NAME, activation) else {
                        return None;
                    };
                    let name = name.to_string();
                    names.iter().position(|scene| *scene == name)
                }
                SceneList::Array(..) => None,
            });
            Some(Self { names, current })
        })
    }

    /// The index of the scene before the one being shown.
    pub fn previous(&self) -> Option<usize> {
        self.current?.checked_sub(1)
    }

    /// The index of the scene after the one being shown, or of the first scene when the
    /// game doesn't say which one is shown.
    pub fn next(&self) -> Option<usize> {
        let next = self.current.map_or(0, |current| current + 1);
        (next < self.names.len()).then_some(next)
    }

    /// Makes the game whose root timeline is `root` switch to the scene at `index` in
    /// its scene list, the way the game's own controls do.
    pub(crate) fn show<'gc>(
        context: &mut UpdateContext<'gc>,
        root: DisplayObject<'gc>,
        index: usize,
    ) {
        let Some(object) = root.object1() else {
            return;
        };
        let scene = Avm1::run_with_stack_frame_for_display_object(root, context, |activation| {
            let list = SceneList::of(root, object, activation)?;
            if index >= list.len() {
                return None;
            }
            let number = index as f64 + 1.0;
            let show_scene = list.show_scene();
            if let SceneList::Clips(_) | SceneList::Parts(_) = list {
                return Some((list.scene(index, activation), show_scene));
            }
            // Hitomi's version moves on from the current scene by the number it's given, so
            // it gets how far the scene is from that one.
            let current = variable(object, CURRENT_SCENE_TO_MOVE_FROM, activation);
            let scene = match (current, variable(object, SCENE_COUNT, activation)) {
                (Value::Number(current), _) => Value::Number(number - current),
                (_, Value::Number(_)) => Value::Number(number),
                _ => list.scene(index, activation),
            };
            Some((scene, show_scene))
        });
        let Some((scene, show_scene)) = scene else {
            return;
        };

        // The games' own controls stop the sounds of the scene they leave, and show the
        // mouse cursor, which some scenes hide.
        context.stop_all_sounds();
        context.ui.set_mouse_visible(true);
        let show_scene = AvmString::new_utf8(context.gc(), show_scene);
        Avm1::run_stack_frame_for_method(root, object, show_scene, &[scene], context);
    }
}

/// The value stored in the variable `name` of `object`, without running any code.
fn stored<'gc>(
    object: Avm1Object<'gc>,
    name: &str,
    activation: &mut Activation<'_, 'gc>,
) -> Value<'gc> {
    let name = AvmString::new_utf8(activation.gc(), name);
    object
        .get_stored(name, activation)
        .unwrap_or(Value::Undefined)
}

/// Whether `value` is a function.
fn is_function(value: Value<'_>) -> bool {
    matches!(value, Value::Object(object) if object.as_function().is_some())
}

/// The number of a scene named like `screen1` or `screen_1`, in lowercase.
fn scene_number(lower_name: &str) -> Option<u32> {
    let rest = lower_name.strip_prefix(SCENE_CLIP_PREFIX)?;
    let digits = rest.strip_prefix('_').unwrap_or(rest);
    (!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
}

/// The value of the engine's variable `name`: the root timeline `object`'s, or else
/// `_global`'s, without running any code.
fn variable<'gc>(
    object: Avm1Object<'gc>,
    name: &str,
    activation: &mut Activation<'_, 'gc>,
) -> Value<'gc> {
    match stored(object, name, activation) {
        Value::Undefined => stored(activation.global_object(), name, activation),
        value => value,
    }
}

/// The scenes of a game.
enum SceneList<'gc> {
    /// The array in one of the [`SCENE_LISTS`] variables, and how many of its scenes to read.
    Array(Avm1Object<'gc>, i32),

    /// The export names and IDs of the movie clips of the scenes, in the order the game
    /// numbers them, in the version of the engine that has no scene list.
    Clips(Vec<(AvmString<'gc>, CharacterId)>),

    /// The names of the scenes that the parts in [`SCENE_PARTS`] reported, in the version
    /// of the engine that loads them all at once.
    Parts(Vec<AvmString<'gc>>),
}

impl<'gc> SceneList<'gc> {
    /// The scenes of the game whose root timeline is `root`, with `object`, if it has some,
    /// and the function that shows them.
    fn of(
        root: DisplayObject<'gc>,
        object: Avm1Object<'gc>,
        activation: &mut Activation<'_, 'gc>,
    ) -> Option<Self> {
        if is_function(stored(object, SHOW_SCENE_IN_PARTS, activation)) {
            return Self::parts(object, activation);
        }
        if !is_function(stored(object, SHOW_SCENE, activation)) {
            return None;
        }
        let array = SCENE_LISTS
            .iter()
            .find_map(|name| match variable(object, name, activation) {
                Value::Object(list) => {
                    let length = list.length(activation).ok().filter(|length| *length > 0)?;
                    Some(Self::Array(list, length.min(MAX_GAME_SCENES)))
                }
                _ => None,
            });
        let clips = |activation: &mut Activation<'_, 'gc>| {
            let clips = scene_clips(root, activation.context);
            (!clips.is_empty()).then_some(Self::Clips(clips))
        };
        match array {
            // A launcher can leave its list in _global for the parts it loads over itself,
            // which export clips of its names, like Big Queen 1's. One Piece of Luck's part,
            // of the version without a list, exports screen1... while its launcher's list
            // names screen_0..., which it doesn't have, so its clips are its scenes.
            Some(list) if !list.names_clip_of(root, activation) => clips(activation).or(Some(list)),
            None => clips(activation),
            list => list,
        }
    }

    /// Whether a scene of an `Array` list is the export name of a movie clip of `root`'s
    /// movie. The others always are.
    fn names_clip_of(
        &self,
        root: DisplayObject<'gc>,
        activation: &mut Activation<'_, 'gc>,
    ) -> bool {
        let Self::Array(list, length) = self else {
            return true;
        };
        (0..*length).any(|index| {
            let Value::String(name) = list.get_element(activation, index) else {
                return false;
            };
            activation
                .context
                .library
                .library_for_movie(root.movie())
                .and_then(|library| library.character_by_export_name(&name))
                .is_some_and(|(_, character)| matches!(character, Character::MovieClip(_)))
        })
    }

    /// The names of the scenes that the parts in [`SCENE_PARTS`] of the root timeline
    /// `object` reported, sorted by their numbers when they all have one.
    fn parts(object: Avm1Object<'gc>, activation: &mut Activation<'_, 'gc>) -> Option<Self> {
        let Value::Object(parts) = stored(object, SCENE_PARTS, activation) else {
            return None;
        };
        let mut names = Vec::new();
        let part_count = parts.length(activation).unwrap_or(0).min(MAX_GAME_SCENES);
        for part in 0..part_count {
            let Value::Object(part) = parts.get_element(activation, part) else {
                continue;
            };
            let Value::Object(screens) = stored(part, "screens", activation) else {
                continue;
            };
            let screen_count = screens.length(activation).unwrap_or(0).min(MAX_GAME_SCENES);
            for screen in 0..screen_count {
                if let Value::String(name) = screens.get_element(activation, screen) {
                    names.push(name);
                }
            }
        }
        let numbers: Option<Vec<_>> = names
            .iter()
            .map(|name| scene_number(&name.to_string().to_ascii_lowercase()))
            .collect();
        if let Some(numbers) = numbers {
            let mut numbered: Vec<_> = numbers.into_iter().zip(names).collect();
            numbered.sort_by_key(|(number, _)| *number);
            names = numbered.into_iter().map(|(_, name)| name).collect();
        }
        (!names.is_empty()).then_some(Self::Parts(names))
    }

    /// The function of the root timeline that shows a scene.
    fn show_scene(&self) -> &'static str {
        match self {
            Self::Parts(_) => SHOW_SCENE_IN_PARTS,
            _ => SHOW_SCENE,
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Array(_, length) => *length as usize,
            Self::Clips(clips) => clips.len(),
            Self::Parts(names) => names.len(),
        }
    }

    /// The scene at `index` (less than [`Self::len`]), as it's listed.
    fn scene(&self, index: usize, activation: &mut Activation<'_, 'gc>) -> Value<'gc> {
        match self {
            Self::Array(list, _) => list.get_element(activation, index as i32),
            Self::Clips(clips) => Value::String(clips[index].0),
            Self::Parts(names) => Value::String(names[index]),
        }
    }
}

/// The export names and IDs of the movie clips that the root timeline `root` exports for
/// its scenes, in the version of the engine that has no scene list: the main menu, then
/// `screen1`, `screen2`... (or `screen_1`, `screen_2`... in some games, like Universal
/// Soldier), then the others whose names start with `screen_`. There are none unless
/// numbered ones are there.
fn scene_clips<'gc>(
    root: DisplayObject<'gc>,
    context: &UpdateContext<'gc>,
) -> Vec<(AvmString<'gc>, CharacterId)> {
    let Some(library) = context.library.library_for_movie(root.movie()) else {
        return Vec::new();
    };
    let mut clips: Vec<_> = library
        .export_characters()
        .iter()
        .filter(|(_, id)| matches!(library.character_by_id(**id), Some(Character::MovieClip(_))))
        .filter_map(|(name, id)| {
            let lower = name.to_string().to_ascii_lowercase();
            let order = if MAIN_MENU_CLIPS.contains(&lower.as_str()) {
                (0, 0)
            } else {
                let rest = lower.strip_prefix(SCENE_CLIP_PREFIX)?;
                match scene_number(&lower) {
                    Some(number) => (1, number),
                    None if rest.starts_with('_') => (2, 0),
                    None => return None,
                }
            };
            Some((order, lower, name, *id))
        })
        .collect();
    if !clips.iter().any(|((group, _), ..)| *group == 1) {
        return Vec::new();
    }
    clips.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    clips
        .into_iter()
        .map(|(_, _, name, id)| (name, id))
        .collect()
}

/// The 1-based frames that the code in the tag stream `data` of a movie jumps its root
/// timeline to: `gotoAndPlay(n)` and `gotoAndStop(n)` in the code of the root timeline
/// and of the buttons placed on it, and `_root.gotoAndPlay(n)` (or `_level0`, or inside
/// `tellTarget("_root")`) anywhere. Unreadable code is skipped.
fn frame_jumps(data: &[u8], version: u8) -> Vec<u16> {
    let mut jumps = Vec::new();
    for (code, on_root) in code_blocks(data, version) {
        scan_jumps(code, version, on_root, &mut jumps);
    }
    jumps
}

/// The blocks of AVM1 code in the tag stream `data` of a movie, in the order they come
/// in, each with whether its own target is the root timeline: the code of the root
/// timeline's frames and of the buttons placed on it. The others' are movie clips: the
/// code of their frames, their `onClipEvent` handlers and `#initclip` code, and the
/// buttons placed in them. Unreadable tags are skipped.
fn code_blocks(data: &[u8], version: u8) -> Vec<(&[u8], bool)> {
    /// Whose code a block is.
    enum Owner {
        Root,
        Clip,
        Button(CharacterId),
    }
    fn add_clip_actions<'a>(
        place_object: &swf::PlaceObject<'a>,
        blocks: &mut Vec<(&'a [u8], Owner)>,
    ) {
        for clip_action in place_object
            .clip_actions
            .iter()
            .flat_map(|actions| &actions.records)
        {
            blocks.push((clip_action.action_data, Owner::Clip));
        }
    }

    let mut blocks = Vec::new();
    let mut placed_on_root = HashSet::new();
    let mut reader = swf::read::Reader::new(data, version);
    while let Ok((tag_code, length)) = reader.read_tag_code_and_length() {
        let rest = reader.get_ref();
        let Some(body) = rest.get(..length) else {
            break;
        };
        *reader.get_mut() = &rest[length..];
        let mut tag = swf::read::Reader::new(body, version);
        match TagCode::from_u16(tag_code) {
            Some(TagCode::End) => break,
            Some(TagCode::DoAction) => blocks.push((body, Owner::Root)),
            Some(TagCode::DoInitAction) => {
                blocks.extend(body.get(2..).map(|code| (code, Owner::Clip)));
            }
            Some(code @ (TagCode::DefineButton | TagCode::DefineButton2)) => {
                let button = if code == TagCode::DefineButton {
                    tag.read_define_button_1()
                } else {
                    tag.read_define_button_2()
                };
                if let Ok(button) = button {
                    for action in &button.actions {
                        blocks.push((action.action_data, Owner::Button(button.id)));
                    }
                }
            }
            Some(code @ (TagCode::PlaceObject | TagCode::PlaceObject2 | TagCode::PlaceObject3)) => {
                let place_object = match code {
                    TagCode::PlaceObject => tag.read_place_object(),
                    TagCode::PlaceObject2 => tag.read_place_object_2_or_3(2),
                    _ => tag.read_place_object_2_or_3(3),
                };
                if let Ok(place_object) = place_object {
                    if let swf::PlaceObjectAction::Place(id) | swf::PlaceObjectAction::Replace(id) =
                        place_object.action
                    {
                        placed_on_root.insert(id);
                    }
                    add_clip_actions(&place_object, &mut blocks);
                }
            }
            Some(TagCode::DefineSprite) => {
                for sprite_tag in tag
                    .read_define_sprite()
                    .map(|sprite| sprite.tags)
                    .unwrap_or_default()
                {
                    match sprite_tag {
                        Tag::DoAction(code) => blocks.push((code, Owner::Clip)),
                        Tag::PlaceObject(place_object) => {
                            add_clip_actions(&place_object, &mut blocks)
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    blocks
        .into_iter()
        .map(|(code, owner)| {
            let on_root = match owner {
                Owner::Root => true,
                Owner::Clip => false,
                Owner::Button(id) => placed_on_root.contains(&id),
            };
            (code, on_root)
        })
        .collect()
}

/// Adds the frames that the AVM1 `code` jumps the root timeline to to `jumps`, where
/// `on_root` is whether the code's own target is the root timeline.
fn scan_jumps(code: &[u8], version: u8, on_root: bool, jumps: &mut Vec<u16>) {
    /// A value on the stack, as far as it's followed.
    enum Item<'a> {
        Number(f64),
        String(&'a SwfStr),
        Variable(&'a SwfStr),
        Other,
    }
    let is_root = |path: &SwfStr| {
        ["_root", "_level0", "/", "_root/", "_level0/"]
            .iter()
            .any(|root| path.as_bytes().eq_ignore_ascii_case(root.as_bytes()))
    };
    let add = |jumps: &mut Vec<u16>, frame: f64| {
        if frame.fract() == 0.0 && (1.0..=f64::from(u16::MAX)).contains(&frame) {
            jumps.push(frame as u16);
        }
    };

    let mut reader = swf::avm1::read::Reader::new(code, version);
    let mut constants: Vec<&SwfStr> = Vec::new();
    let mut stack = Vec::new();
    let mut target_is_root = on_root;
    while let Ok(action) = reader.read_action() {
        match action {
            Action::End => break,
            Action::ConstantPool(pool) => constants = pool.strings,
            Action::Push(push) => stack.extend(push.values.iter().map(|value| {
                match value {
                    Avm1Value::Int(number) => Item::Number(f64::from(*number)),
                    Avm1Value::Float(number) => Item::Number(f64::from(*number)),
                    Avm1Value::Double(number) => Item::Number(*number),
                    Avm1Value::Str(string) => Item::String(string),
                    Avm1Value::ConstantPool(index) => constants
                        .get(usize::from(*index))
                        .map_or(Item::Other, |string| Item::String(string)),
                    _ => Item::Other,
                }
            })),
            Action::GetVariable => {
                let variable = match stack.pop() {
                    Some(Item::String(name)) => Item::Variable(name),
                    _ => Item::Other,
                };
                stack.push(variable);
            }
            Action::GotoFrame(goto) if target_is_root => add(jumps, f64::from(goto.frame) + 1.0),
            Action::GotoFrame2(goto) => {
                if let (true, Some(Item::Number(frame))) = (target_is_root, stack.pop()) {
                    add(jumps, frame + f64::from(goto.scene_offset));
                }
                stack.clear();
            }
            Action::SetTarget(set_target) => {
                target_is_root = if set_target.target.is_empty() {
                    on_root
                } else {
                    is_root(set_target.target)
                };
            }
            Action::CallMethod => {
                let name = stack.pop();
                let object = stack.pop();
                let count = stack.pop();
                let frame = stack.pop();
                if let (
                    Some(Item::String(name)),
                    Some(Item::Variable(object)),
                    Some(Item::Number(count)),
                    Some(Item::Number(frame)),
                ) = (name, object, count, frame)
                    && is_root(object)
                    && count == 1.0
                    && [b"gotoAndPlay".as_slice(), b"gotoAndStop"].contains(&name.as_bytes())
                {
                    add(jumps, frame);
                }
                stack.clear();
            }
            Action::DefineFunction(function) => {
                scan_jumps(function.actions, version, on_root, jumps);
                stack.clear();
            }
            Action::DefineFunction2(function) => {
                scan_jumps(function.actions, version, on_root, jumps);
                stack.clear();
            }
            Action::With(with) => {
                scan_jumps(with.actions, version, target_is_root, jumps);
                stack.clear();
            }
            Action::Try(try_block) => {
                for body in [
                    Some(try_block.try_body),
                    try_block.catch_body.map(|(_, body)| body),
                ]
                .into_iter()
                .chain([try_block.finally_body])
                .flatten()
                {
                    scan_jumps(body, version, target_is_root, jumps);
                }
                stack.clear();
            }
            _ => stack.clear(),
        }
    }
}

/// Where a [`FrameVariable`] lives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Home {
    /// The root timeline
    Root,
    /// `_global`
    Global,
}

/// A variable that the code of a movie picks the frame of a timeline with, like the
/// version of a character, picked earlier in the game, in `gotoAndStop(_root.version)`.
/// A scene opened from the scene menu skips the code that gives it a value, which leaves
/// such clips playing through all their versions, so the menu gives it the first constant
/// that the movie's code gives a variable of its name, or else 1, the first frame.
#[derive(Clone, Debug, PartialEq)]
struct FrameVariable {
    home: Home,
    name: String,
    value: Constant,
}

#[derive(Clone, Debug, PartialEq)]
enum Constant {
    Number(f64),
    String(String),
    Bool(bool),
}

/// The ways that the code of a movie clip picks the frame it shows, one of its versions,
/// with conditions, like Gotham City's, which shows the person placed next to a girl with
/// `if (_root.person1nearGirl1 == true) gotoAndStop(1)`, and so on for the others. A scene
/// opened from the scene menu skips the code that makes one of them hold, which leaves the
/// clip playing through all its versions, so the menu makes the first one hold.
#[derive(Clone, Debug, Default, PartialEq)]
struct Versions {
    /// That a variable has a value, for each frame it goes to
    conditions: Vec<(Home, String, Constant)>,
    /// Whether the clip stops by itself, without any of them holding
    stops: bool,
}

/// What the scene menu gives values to before it jumps.
#[derive(Debug, Default, PartialEq)]
struct FramePicks {
    variables: Vec<FrameVariable>,
    versions: Vec<Versions>,
}

/// The timeline that code scanned for [`FrameVariable`]s works on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Target {
    Root,
    /// A movie clip, whose variables can't be given values before it's there
    Clip,
    /// One that a path names, or that isn't known
    Other,
}

/// A value on the stack of code scanned for [`FrameVariable`]s, as far as it's followed.
#[derive(Clone, Debug)]
enum Item {
    Constant(Constant),
    /// A timeline whose variables can be [`FrameVariable`]s
    Timeline(Home),
    /// A value worked out from these variables
    Reads(Vec<(Home, String)>),
    /// Whether a variable has a value, or doesn't, if `holds` is false
    Test {
        variable: (Home, String),
        value: Constant,
        holds: bool,
    },
    Other,
}

impl Item {
    fn reads(self) -> Vec<(Home, String)> {
        match self {
            Item::Reads(reads) => reads,
            Item::Test { variable, .. } => vec![variable],
            _ => Vec::new(),
        }
    }

    /// A value worked out from `items`.
    fn from(items: Vec<Item>) -> Item {
        let reads: Vec<_> = items.into_iter().flat_map(Item::reads).collect();
        if reads.is_empty() {
            Item::Other
        } else {
            Item::Reads(reads)
        }
    }
}

/// What the code of a movie does with the variables that can be [`FrameVariable`]s.
#[derive(Default)]
struct VariableUses {
    /// The variables read to work out the frames that the code goes to
    picks: Vec<(Home, String)>,
    /// The constants that the code gives variables, by name, in the order of the code
    constants: Vec<(String, Constant)>,
    /// The [`Versions`] of each block of a movie clip's code
    versions: Vec<Versions>,
}

/// The [`FrameVariable`]s and [`Versions`] of the movie whose tag stream is `data`.
fn frame_variables(data: &[u8], version: u8) -> FramePicks {
    let mut uses = VariableUses::default();
    for (code, on_root) in code_blocks(data, version) {
        let target = if on_root { Target::Root } else { Target::Clip };
        uses.versions.push(Versions::default());
        scan_variables(code, version, target, false, HashMap::new(), &mut uses);
    }
    // A clip picks versions with more than one condition; one alone could be a game's
    // state, like whether it's paused.
    let mut versions: Vec<Versions> = Vec::new();
    for found in uses.versions {
        if !found.stops && found.conditions.len() > 1 && !versions.contains(&found) {
            versions.push(found);
        }
    }
    let mut variables: Vec<FrameVariable> = Vec::new();
    for (home, name) in uses.picks {
        // Names starting with _ are properties, like _currentframe.
        let known = variables
            .iter()
            .any(|known| known.home == home && known.name.eq_ignore_ascii_case(&name));
        if name.starts_with('_') || known {
            continue;
        }
        let value = uses
            .constants
            .iter()
            .find(|(set, _)| set.eq_ignore_ascii_case(&name))
            .map_or(Constant::Number(1.0), |(_, value)| value.clone());
        variables.push(FrameVariable { home, name, value });
    }
    FramePicks {
        variables,
        versions,
    }
}

/// Where a variable that code working on `target` reads by its name alone lives, if it can
/// be a [`FrameVariable`]: a movie clip that doesn't have it reads `_global`'s, from SWF 6.
fn own_home(target: Target, version: u8) -> Option<Home> {
    match target {
        Target::Root => Some(Home::Root),
        Target::Clip if version >= 6 => Some(Home::Global),
        _ => None,
    }
}

/// The timeline named by `path`, like `_root`, `/` or `_global`, in code working on
/// `target`, if its variables can be [`FrameVariable`]s.
fn timeline_home(path: &str, target: Target) -> Option<Home> {
    let path = path.trim_end_matches(['/', ':']);
    let is = |name: &str| path.eq_ignore_ascii_case(name);
    if path.is_empty() || is("_root") || is("_level0") {
        Some(Home::Root)
    } else if is("_global") {
        Some(Home::Global)
    } else if (is("_parent") && target == Target::Clip) || (is("this") && target == Target::Root) {
        // Movie clips are mostly placed on the root timeline.
        Some(Home::Root)
    } else {
        None
    }
}

/// Where the variable named by `path`, like `version`, `_root.version` or `/:version`,
/// lives, and its name, in code working on `target`, if it can be a [`FrameVariable`].
fn variable_home(path: &str, target: Target, version: u8) -> Option<(Home, &str)> {
    match path.rfind(['.', ':', '/']) {
        None => Some((own_home(target, version)?, path)),
        Some(end) => {
            let name = &path[end + 1..];
            let home = timeline_home(&path[..end], target)?;
            (!name.is_empty()).then_some((home, name))
        }
    }
}

/// Adds what the AVM1 `code`, which works on `own`, does with variables that can be
/// [`FrameVariable`]s to `uses`, where `in_function` is whether it's the body of a
/// function, whose local variables aren't the game's, and `registers` what its registers
/// start with.
fn scan_variables(
    code: &[u8],
    version: u8,
    own: Target,
    in_function: bool,
    mut registers: HashMap<u8, Item>,
    uses: &mut VariableUses,
) {
    let encoding = SwfStr::encoding_for_version(version);
    let constant =
        |string: &SwfStr| Item::Constant(Constant::String(string.to_string_lossy(encoding)));
    let pop = |stack: &mut Vec<Item>| stack.pop().unwrap_or(Item::Other);
    // The number of arguments, or items, that an action pops, if it's known
    let pop_count = |stack: &mut Vec<Item>| match stack.pop() {
        Some(Item::Constant(Constant::Number(count))) if (0.0..=256.0).contains(&count) => {
            Some(count as usize)
        }
        _ => None,
    };
    let is_goto = |name: &Item| {
        matches!(name, Item::Constant(Constant::String(name))
            if name.eq_ignore_ascii_case("gotoAndStop") || name.eq_ignore_ascii_case("gotoAndPlay"))
    };

    // Whether the code is a movie clip's own, which picks the frame it shows, and the
    // condition that the code being read runs under, with where the code it guards ends
    let picks_versions = own == Target::Clip && !in_function;
    let mut guard: Option<((Home, String, Constant), usize)> = None;
    let mut guarded_until = 0;
    // Notes a goto of the code's own timeline to a frame it names, at `position`, which
    // shows a version if a condition guards it, or else stops there.
    let goto = |guard: &mut Option<((Home, String, Constant), usize)>,
                position: usize,
                stops: bool,
                uses: &mut VariableUses| {
        let Some(versions) = uses.versions.last_mut() else {
            return;
        };
        match guard.take() {
            Some((condition, end)) if position < end => versions.conditions.push(condition),
            _ => versions.stops |= stops,
        }
    };

    let mut reader = swf::avm1::read::Reader::new(code, version);
    let mut constants: Vec<&SwfStr> = Vec::new();
    let mut stack: Vec<Item> = Vec::new();
    let mut target = own;
    loop {
        let position = code.len() - reader.get_ref().len();
        let Ok(action) = reader.read_action() else {
            break;
        };
        let next = code.len() - reader.get_ref().len();
        let own_goto = picks_versions && target == own;
        match action {
            Action::End => break,
            Action::ConstantPool(pool) => constants = pool.strings,
            Action::Push(push) => {
                for value in &push.values {
                    stack.push(match value {
                        Avm1Value::Int(number) => {
                            Item::Constant(Constant::Number(f64::from(*number)))
                        }
                        Avm1Value::Float(number) => {
                            Item::Constant(Constant::Number(f64::from(*number)))
                        }
                        Avm1Value::Double(number) => Item::Constant(Constant::Number(*number)),
                        Avm1Value::Str(string) => constant(string),
                        Avm1Value::ConstantPool(index) => constants
                            .get(usize::from(*index))
                            .map_or(Item::Other, |string| constant(string)),
                        Avm1Value::Register(register) => {
                            registers.get(register).cloned().unwrap_or(Item::Other)
                        }
                        Avm1Value::Bool(value) => Item::Constant(Constant::Bool(*value)),
                        _ => Item::Other,
                    });
                }
            }
            Action::GetVariable => {
                let item = match pop(&mut stack) {
                    Item::Constant(Constant::String(path)) => {
                        match (
                            timeline_home(&path, target),
                            variable_home(&path, target, version),
                        ) {
                            (Some(home), _) if !path.is_empty() => Item::Timeline(home),
                            (_, Some((home, name))) => Item::Reads(vec![(home, name.to_owned())]),
                            _ => Item::Other,
                        }
                    }
                    _ => Item::Other,
                };
                stack.push(item);
            }
            Action::GetMember => {
                let member = pop(&mut stack);
                let object = pop(&mut stack);
                stack.push(match (object, member) {
                    (Item::Timeline(home), Item::Constant(Constant::String(name))) => {
                        Item::Reads(vec![(home, name)])
                    }
                    _ => Item::Other,
                });
            }
            Action::SetVariable => {
                let value = pop(&mut stack);
                let path = pop(&mut stack);
                if let (Item::Constant(value), Item::Constant(Constant::String(path))) =
                    (value, path)
                {
                    let name = path.rsplit(['.', ':', '/']).next().unwrap_or_default();
                    uses.constants.push((name.to_owned(), value));
                }
            }
            Action::SetMember => {
                let value = pop(&mut stack);
                let name = pop(&mut stack);
                pop(&mut stack);
                if let (Item::Constant(value), Item::Constant(Constant::String(name))) =
                    (value, name)
                {
                    uses.constants.push((name, value));
                }
            }
            Action::DefineLocal => {
                let value = pop(&mut stack);
                let name = pop(&mut stack);
                if let (false, Item::Constant(value), Item::Constant(Constant::String(name))) =
                    (in_function, value, name)
                {
                    uses.constants.push((name, value));
                }
            }
            Action::DefineLocal2 | Action::Pop | Action::Trace => {
                pop(&mut stack);
            }
            Action::If(branch) => {
                // `if (variable == value) gotoAndStop(n);` jumps past the goto unless the
                // condition holds.
                if let (
                    true,
                    Item::Test {
                        variable,
                        value,
                        holds: false,
                    },
                ) = (own_goto, pop(&mut stack))
                {
                    let end = usize::try_from(next as isize + isize::from(branch.offset))
                        .unwrap_or_default();
                    guard = Some(((variable.0, variable.1, value), end));
                    guarded_until = end;
                }
            }
            Action::Equals | Action::Equals2 | Action::StrictEquals => {
                let right = pop(&mut stack);
                let left = pop(&mut stack);
                stack.push(match (left, right) {
                    (Item::Reads(mut reads), Item::Constant(value))
                    | (Item::Constant(value), Item::Reads(mut reads))
                        if reads.len() == 1 =>
                    {
                        Item::Test {
                            variable: reads.remove(0),
                            value,
                            holds: true,
                        }
                    }
                    (left, right) => Item::from(vec![left, right]),
                });
            }
            Action::Not => {
                let value = pop(&mut stack);
                stack.push(match value {
                    Item::Test {
                        variable,
                        value,
                        holds,
                    } => Item::Test {
                        variable,
                        value,
                        holds: !holds,
                    },
                    // A variable as a condition holds when it's true.
                    Item::Reads(mut reads) if reads.len() == 1 => Item::Test {
                        variable: reads.remove(0),
                        value: Constant::Bool(true),
                        holds: false,
                    },
                    value => Item::from(vec![value]),
                });
            }
            Action::StoreRegister(store) => {
                if let Some(top) = stack.last() {
                    registers.insert(store.register, top.clone());
                }
            }
            Action::PushDuplicate => {
                let top = stack.last().cloned().unwrap_or(Item::Other);
                stack.push(top);
            }
            Action::StackSwap => {
                let top = pop(&mut stack);
                let below = pop(&mut stack);
                stack.extend([top, below]);
            }
            Action::Add
            | Action::Add2
            | Action::Subtract
            | Action::Multiply
            | Action::Divide
            | Action::Modulo
            | Action::StringAdd
            | Action::Less
            | Action::Less2
            | Action::Greater
            | Action::StringEquals
            | Action::StringLess
            | Action::StringGreater
            | Action::And
            | Action::Or
            | Action::BitAnd
            | Action::BitOr
            | Action::BitXor
            | Action::BitLShift
            | Action::BitRShift
            | Action::BitURShift => {
                let right = pop(&mut stack);
                let left = pop(&mut stack);
                stack.push(Item::from(vec![left, right]));
            }
            Action::ToInteger
            | Action::ToNumber
            | Action::ToString
            | Action::Increment
            | Action::Decrement
            | Action::StringLength
            | Action::MBStringLength
            | Action::CharToAscii
            | Action::AsciiToChar
            | Action::MBCharToAscii
            | Action::MBAsciiToChar
            | Action::TypeOf
            | Action::RandomNumber => {
                let value = pop(&mut stack);
                stack.push(Item::from(vec![value]));
            }
            Action::GotoFrame2(goto_frame) => {
                let frame = pop(&mut stack);
                if let (true, Item::Constant(_)) = (own_goto, &frame) {
                    goto(&mut guard, position, !goto_frame.set_playing, uses);
                }
                uses.picks.extend(frame.reads());
            }
            // Like gotoAndStop
            Action::GotoFrame(_) | Action::GotoLabel(_) => {
                if own_goto {
                    goto(&mut guard, position, true, uses);
                }
            }
            Action::Stop => {
                if let (true, Some(versions)) = (
                    own_goto && position >= guarded_until,
                    uses.versions.last_mut(),
                ) {
                    versions.stops = true;
                }
            }
            Action::CallFunction | Action::CallMethod | Action::NewObject | Action::NewMethod => {
                let is_call = matches!(action, Action::CallFunction | Action::CallMethod);
                let name = pop(&mut stack);
                if matches!(action, Action::CallMethod | Action::NewMethod) {
                    pop(&mut stack);
                }
                let Some(count) = pop_count(&mut stack) else {
                    stack.clear();
                    continue;
                };
                let mut args: Vec<Item> = (0..count).map(|_| pop(&mut stack)).collect();
                if is_call && is_goto(&name) && !args.is_empty() {
                    uses.picks.extend(args.swap_remove(0).reads());
                }
                stack.push(Item::Other);
            }
            Action::InitArray | Action::InitObject => {
                let per_item = if matches!(action, Action::InitObject) {
                    2
                } else {
                    1
                };
                let Some(count) = pop_count(&mut stack) else {
                    stack.clear();
                    continue;
                };
                for _ in 0..count * per_item {
                    pop(&mut stack);
                }
                stack.push(Item::Other);
            }
            Action::Play
            | Action::NextFrame
            | Action::PreviousFrame
            | Action::Jump(_)
            | Action::StopSounds
            | Action::ToggleQuality
            | Action::EndDrag => {}
            Action::SetTarget(set_target) => {
                let path = set_target.target.to_string_lossy(encoding);
                target = if path.is_empty() {
                    own
                } else if timeline_home(&path, own) == Some(Home::Root) {
                    Target::Root
                } else {
                    Target::Other
                };
            }
            Action::SetTarget2 => {
                pop(&mut stack);
                target = Target::Other;
            }
            Action::DefineFunction(function) => {
                scan_variables(
                    function.actions,
                    version,
                    target,
                    true,
                    HashMap::new(),
                    uses,
                );
                if function.name.is_empty() {
                    stack.push(Item::Other);
                }
            }
            Action::DefineFunction2(function) => {
                // The values that a function preloads go in its registers from 1, in this order.
                let timeline =
                    |path| timeline_home(path, target).map_or(Item::Other, Item::Timeline);
                let preloaded = [
                    (FunctionFlags::PRELOAD_THIS, timeline("this")),
                    (FunctionFlags::PRELOAD_ARGUMENTS, Item::Other),
                    (FunctionFlags::PRELOAD_SUPER, Item::Other),
                    (FunctionFlags::PRELOAD_ROOT, Item::Timeline(Home::Root)),
                    (FunctionFlags::PRELOAD_PARENT, timeline("_parent")),
                    (FunctionFlags::PRELOAD_GLOBAL, Item::Timeline(Home::Global)),
                ];
                let function_registers = preloaded
                    .into_iter()
                    .filter(|(flag, _)| function.flags.contains(*flag))
                    .zip(1..)
                    .map(|((_, item), register)| (register, item))
                    .collect();
                scan_variables(
                    function.actions,
                    version,
                    target,
                    true,
                    function_registers,
                    uses,
                );
                if function.name.is_empty() {
                    stack.push(Item::Other);
                }
            }
            Action::With(with) => {
                pop(&mut stack);
                scan_variables(
                    with.actions,
                    version,
                    Target::Other,
                    in_function,
                    registers.clone(),
                    uses,
                );
            }
            Action::Try(try_block) => {
                for body in [
                    Some(try_block.try_body),
                    try_block.catch_body.map(|(_, body)| body),
                ]
                .into_iter()
                .chain([try_block.finally_body])
                .flatten()
                {
                    scan_variables(body, version, target, in_function, registers.clone(), uses);
                }
            }
            _ => stack.clear(),
        }
    }
}

/// Gives the [`FrameVariable`]s that have no value their value, and makes the first
/// condition of [`Versions`] that none of hold, before the scene menu jumps over the code
/// that would. They're read from all the loaded movies, as the parts of games that load
/// their scenes from other files use the root timeline's variables too.
pub(crate) fn fill_frame_variables<'gc>(
    context: &mut UpdateContext<'gc>,
    root: DisplayObject<'gc>,
) {
    let Some(object) = root.object1() else {
        return;
    };
    let mut picks = FramePicks::default();
    for movie in context.library.known_movies() {
        if movie.is_action_script_3() {
            continue;
        }
        let found = frame_variables(movie.data(), movie.version());
        picks.variables.extend(found.variables);
        picks.versions.extend(found.versions);
    }
    if picks.variables.is_empty() && picks.versions.is_empty() {
        return;
    }
    Avm1::run_with_stack_frame_for_display_object(root, context, |activation| {
        let home_of = |home: Home, activation: &mut Activation<'_, 'gc>| match home {
            Home::Root => object,
            Home::Global => activation.global_object(),
        };
        let set = |home: Avm1Object<'gc>,
                   name: &str,
                   value: &Constant,
                   activation: &mut Activation<'_, 'gc>| {
            let name = AvmString::new_utf8(activation.gc(), name);
            let value = match value {
                Constant::Number(number) => Value::Number(*number),
                Constant::String(string) => AvmString::new_utf8(activation.gc(), string).into(),
                Constant::Bool(value) => Value::Bool(*value),
            };
            // A setter of the game's that fails leaves the variable as it is.
            let _ = home.set(name, value, activation);
        };
        let global = activation.global_object();
        for variable in &picks.variables {
            let home = home_of(variable.home, activation);
            // One that _global has, like NaN (a getter of its), isn't missing: code that reads
            // it by its name alone finds it there.
            let name = AvmString::new_utf8(activation.gc(), &variable.name);
            if matches!(stored(home, &variable.name, activation), Value::Undefined)
                && !global.has_property(activation, name)
            {
                set(home, &variable.name, &variable.value, activation);
            }
        }
        for versions in &picks.versions {
            let held = versions.conditions.iter().any(|(home, name, value)| {
                let home = home_of(*home, activation);
                holds(stored(home, name, activation), value)
            });
            if let (false, Some((home, name, value))) = (held, versions.conditions.first()) {
                let home = home_of(*home, activation);
                set(home, name, value, activation);
            }
        }
    });
}

/// Whether `value` equals `constant`, the way the games compare them: numbers with true
/// and false like 1 and 0.
fn holds(value: Value<'_>, constant: &Constant) -> bool {
    let number = |value: bool| if value { 1.0 } else { 0.0 };
    match (value, constant) {
        (Value::Bool(value), Constant::Bool(constant)) => value == *constant,
        (Value::Number(value), Constant::Number(constant)) => value == *constant,
        (Value::Bool(value), Constant::Number(constant)) => number(value) == *constant,
        (Value::Number(value), Constant::Bool(constant)) => value == number(*constant),
        (Value::String(value), Constant::String(constant)) => value.to_string() == *constant,
        _ => false,
    }
}

impl RootTimeline {
    pub(crate) fn from_movie_clip(mc: MovieClip<'_>) -> Self {
        let total_frames = mc.header_frames();
        let frames_loaded = mc.frames_loaded().clamp(0, i32::from(total_frames)) as u16;

        let scenes = mc
            .scenes()
            .into_iter()
            .map(|scene| TimelineScene {
                name: scene.name.to_string(),
                start: scene.start,
                length: scene.length,
            })
            .collect();

        let labels = mc
            .labels_in_range(0, u16::MAX)
            .into_iter()
            .map(|(name, frame)| TimelineLabel {
                name: name.to_string(),
                frame,
            })
            .collect();

        Self {
            scenes,
            scenes_are_frame_jumps: false,
            labels,
            current_frame: mc.current_frame(),
            total_frames,
            frames_loaded,
            game_scenes: None,
        }
    }

    /// Makes the frames that the code of `movie`, the root timeline's, jumps the root
    /// timeline to into its `scenes`, starting with the first frame, if it has no other
    /// way to navigate. Call it after `game_scenes` is read.
    pub(crate) fn add_frame_jumps(&mut self, movie: &SwfMovieData) {
        if self.game_scenes.is_some() || self.scenes.len() > 1 || !self.labels.is_empty() {
            return;
        }
        let mut frames = frame_jumps(movie.data(), movie.version());
        frames.retain(|frame| (1..=self.total_frames).contains(frame));
        frames.push(1);
        frames.sort_unstable();
        frames.dedup();
        if frames.len() < 2 {
            return;
        }
        self.scenes = frames
            .iter()
            .enumerate()
            .map(|(index, &start)| {
                let end = frames
                    .get(index + 1)
                    .map_or(self.total_frames, |next| next - 1);
                TimelineScene {
                    name: format!("Frame {start}"),
                    start,
                    length: end - start + 1,
                }
            })
            .collect();
        self.scenes_are_frame_jumps = true;
    }

    /// The index into `scenes` of the scene containing the playhead.
    pub fn current_scene(&self) -> Option<usize> {
        self.scenes
            .iter()
            .rposition(|scene| scene.start <= self.current_frame)
    }

    /// The scene before the one containing the playhead.
    pub fn previous_scene(&self) -> Option<&TimelineScene> {
        let current = self.current_scene()?;
        self.scenes.get(current.checked_sub(1)?)
    }

    /// The scene after the one containing the playhead.
    pub fn next_scene(&self) -> Option<&TimelineScene> {
        match self.current_scene() {
            Some(current) => self.scenes.get(current + 1),
            None => self.scenes.first(),
        }
    }

    /// Whether the given frame has finished loading and can be jumped to.
    pub fn is_frame_loaded(&self, frame: u16) -> bool {
        frame <= self.frames_loaded
    }
}

#[cfg(test)]
mod tests {
    use crate::limits::ExecutionLimit;
    use crate::player::{Player, PlayerBuilder};
    use crate::tag_utils::SwfMovieData;
    use std::sync::{Arc, Mutex};
    use swf::avm1::types::{Action, DefineFunction, Push, Value};

    fn load_test_swf(name: &str) -> Arc<Mutex<Player>> {
        let path = format!(
            "{}/../tests/tests/swfs/{name}/test.swf",
            env!("CARGO_MANIFEST_DIR")
        );
        load_swf(&std::fs::read(path).expect("test SWF should exist"))
    }

    fn load_swf(data: &[u8]) -> Arc<Mutex<Player>> {
        let movie = SwfMovieData::from_data(data, "file:///test.swf".into(), None, None)
            .expect("test SWF should parse");
        let player = PlayerBuilder::new().with_movie(movie).build();
        {
            let mut player = player.lock().unwrap();
            player.preload(&mut ExecutionLimit::none());
            player.run_frame();
        }
        player
    }

    #[test]
    fn navigate_avm2_scenes() {
        let player = load_test_swf("avm2/movieclip_scenes");
        let mut player = player.lock().unwrap();

        let timeline = player.root_timeline().expect("root should be a movie clip");
        let scenes: Vec<_> = timeline
            .scenes
            .iter()
            .map(|scene| (scene.name.as_str(), scene.start, scene.end()))
            .collect();
        assert_eq!(scenes, [("Scene 1", 1, 3), ("Scene 2", 4, 4)]);
        let labels: Vec<_> = timeline
            .labels
            .iter()
            .map(|label| (label.name.as_str(), label.frame))
            .collect();
        assert_eq!(labels, [("frame1", 1), ("frame3", 3), ("frame4", 4)]);
        assert_eq!(timeline.total_frames, 4);
        assert_eq!(timeline.frames_loaded, 4);
        assert_eq!(timeline.current_frame, 1);
        assert_eq!(timeline.current_scene(), Some(0));
        assert!(timeline.previous_scene().is_none());

        let next = timeline.next_scene().expect("Scene 2 follows Scene 1");
        player.goto_and_play_root_frame(next.start);
        let timeline = player.root_timeline().unwrap();
        assert_eq!(timeline.current_frame, 4);
        assert_eq!(timeline.current_scene(), Some(1));
        assert!(timeline.next_scene().is_none());

        let previous = timeline.previous_scene().expect("Scene 1 precedes Scene 2");
        player.goto_and_play_root_frame(previous.start);
        let timeline = player.root_timeline().unwrap();
        assert_eq!(timeline.current_frame, 1);
        assert_eq!(timeline.current_scene(), Some(0));
        assert!(timeline.game_scenes.is_none());
    }

    #[test]
    fn navigate_avm1_labels() {
        let player = load_test_swf("avm1/goto_label");
        let mut player = player.lock().unwrap();

        let timeline = player.root_timeline().expect("root should be a movie clip");
        assert!(timeline.scenes.is_empty());
        assert_eq!(timeline.current_scene(), None);
        let labels: Vec<_> = timeline
            .labels
            .iter()
            .map(|label| (label.name.as_str(), label.frame))
            .collect();
        // Duplicate labels are kept, since jumps go by frame number.
        assert_eq!(
            labels,
            [("someframe", 3), ("end", 5), ("someframe", 6), ("badà", 7)]
        );

        player.goto_and_play_root_frame(3);
        assert_eq!(player.root_timeline().unwrap().current_frame, 3);
        player.goto_and_play_root_frame(6);
        assert_eq!(player.root_timeline().unwrap().current_frame, 6);
        assert!(player.root_timeline().unwrap().game_scenes.is_none());
    }

    /// How a test movie's `setScreen` picks the scene to show.
    #[derive(Clone, Copy, Debug)]
    enum ShowScene {
        /// It takes the name of the scene.
        ByName,
        /// It takes the 1-based number of the scene, and the number of scenes is in `Qfrm`.
        ByNumber,
        /// It moves on by the number of scenes it's given from the one numbered in `Qmod`,
        /// and the number of scenes is in `Qfrm`.
        ByStep,
        /// There's no `setScreen`.
        Missing,
    }

    /// A movie that keeps its scene list like the games [`super::GameScenes`] is for, in
    /// the variable `list`, with the number of the scene being shown in `current` if
    /// given.
    fn game_with_scene_list(
        list: &'static str,
        current: Option<&'static str>,
        show: ShowScene,
        global: bool,
    ) -> Vec<u8> {
        let show_scene = match show {
            // screen_name = name;
            ShowScene::ByName => actions(&[
                push(&[string("screen_name"), string("name")]),
                Action::GetVariable,
                Action::SetVariable,
            ]),
            // Qmod = Qmod + name;
            ShowScene::ByStep => actions(&[
                push(&[string("Qmod"), string("Qmod")]),
                Action::GetVariable,
                push(&[string("name")]),
                Action::GetVariable,
                Action::Add2,
                Action::SetVariable,
            ]),
            // screen_name = <list>[name - 1];
            _ => actions(&[
                push(&[string("screen_name"), string(list)]),
                Action::GetVariable,
                push(&[string("name")]),
                Action::GetVariable,
                push(&[Value::Int(1)]),
                Action::Subtract,
                Action::GetMember,
                Action::SetVariable,
            ]),
        };
        // var <list> = ["intro", "fight", "ending"]; var <current> = 1; var screen_name;
        // function setScreen(name) { ... }
        // With `global`, the list and the current number are `_global.<list>` and
        // `_global.<current>`.
        let define = |variable: &'static str, value: &[Value<'static>], array: bool| {
            let mut define = Vec::new();
            if global {
                define.extend([push(&[string("_global")]), Action::GetVariable]);
            }
            define.push(push(&[&[string(variable)], value].concat()));
            if array {
                define.push(Action::InitArray);
            }
            define.push(if global {
                Action::SetMember
            } else {
                Action::DefineLocal
            });
            define
        };
        let scenes = [
            string("ending"),
            string("fight"),
            string("intro"),
            Value::Int(3),
        ];
        let mut setup = define(list, &scenes, true);
        setup.extend([push(&[string("screen_name")]), Action::DefineLocal2]);
        if let Some(current) = current {
            setup.extend(define(current, &[Value::Int(1)], false));
        }
        if matches!(show, ShowScene::ByNumber | ShowScene::ByStep) {
            setup.extend([push(&[string("Qfrm"), Value::Int(3)]), Action::DefineLocal]);
        }
        if !matches!(show, ShowScene::Missing) {
            setup.push(Action::DefineFunction(DefineFunction {
                name: "setScreen".into(),
                params: vec!["name".into()],
                actions: &show_scene,
            }));
        }
        let mut setup = actions(&setup);
        setup.push(0); // End
        movie(&[swf::Tag::DoAction(&setup), swf::Tag::ShowFrame])
    }

    /// A movie that exports an empty movie clip under each of `exports`, and has a
    /// `setScreen(name)` that attaches the clip exported as `name` as `screen`, which it
    /// does with `mainmenu` first, like the games without a scene list.
    fn game_with_scene_clips(exports: &[&'static str]) -> Vec<u8> {
        game_with_scene_clips_and_list(exports, "_global", "screens", &[])
    }

    /// Like [`game_with_scene_clips`], with `list` in `timeline.variable` too: like the list
    /// that One Piece of Luck's launcher leaves in `_global.screens` when it loads its part
    /// over itself, or One Piece Nami's numbers in `_root.Qvar`, which aren't scenes.
    fn game_with_scene_clips_and_list(
        exports: &[&'static str],
        timeline: &'static str,
        variable: &'static str,
        list: &[Value<'static>],
    ) -> Vec<u8> {
        // attachMovie(name, "screen", 0);
        let show_scene = actions(&[
            push(&[Value::Int(0), string("screen"), string("name")]),
            Action::GetVariable,
            push(&[Value::Int(3), string("attachMovie")]),
            Action::CallFunction,
            Action::Pop,
        ]);
        // _global.screens = [...]; function setScreen(name) { ... } setScreen("mainmenu");
        let mut setup = Vec::new();
        if !list.is_empty() {
            let mut values = vec![string(variable)];
            values.extend(list.iter().rev().cloned());
            values.push(Value::Int(list.len() as i32));
            setup = actions(&[
                push(&[string(timeline)]),
                Action::GetVariable,
                push(&values),
                Action::InitArray,
                Action::SetMember,
            ]);
        }
        setup.extend(actions(&[
            Action::DefineFunction(DefineFunction {
                name: "setScreen".into(),
                params: vec!["name".into()],
                actions: &show_scene,
            }),
            push(&[string("mainmenu"), Value::Int(1), string("setScreen")]),
            Action::CallFunction,
            Action::Pop,
        ]));
        setup.push(0); // End

        let mut tags: Vec<_> = (1..)
            .zip(exports)
            .map(|(id, _)| {
                swf::Tag::DefineSprite(swf::Sprite {
                    id,
                    num_frames: 1,
                    tags: vec![swf::Tag::ShowFrame],
                })
            })
            .collect();
        tags.push(swf::Tag::ExportAssets(
            (1..)
                .zip(exports)
                .map(|(id, name)| swf::ExportedAsset {
                    id,
                    name: (*name).into(),
                })
                .collect(),
        ));
        tags.extend([swf::Tag::DoAction(&setup), swf::Tag::ShowFrame]);
        movie(&tags)
    }

    fn actions(actions: &[Action<'_>]) -> Vec<u8> {
        let mut code = Vec::new();
        let mut writer = swf::avm1::write::Writer::new(&mut code, 8);
        for action in actions {
            writer.write_action(action).unwrap();
        }
        code
    }

    fn push(values: &[Value<'static>]) -> Action<'static> {
        Action::Push(Push {
            values: values.to_vec(),
        })
    }

    fn string(string: &'static str) -> Value<'static> {
        Value::Str(string.into())
    }

    fn movie(tags: &[swf::Tag<'_>]) -> Vec<u8> {
        let header = swf::Header {
            num_frames: 1,
            ..swf::Header::default_with_swf_version(8)
        };
        let mut data = Vec::new();
        swf::write_swf(&header, tags, &mut data).unwrap();
        data
    }

    #[test]
    fn navigate_game_scenes() {
        for (list, current, show, global) in [
            ("scenelist", Some("pos"), ShowScene::ByName, false),
            ("Qvar", Some("Qpos"), ShowScene::ByName, false),
            ("Qvar", None, ShowScene::ByNumber, false),
            ("Qvar", Some("Qmod"), ShowScene::ByStep, false),
            // Big Queen 1's and Evil Sorceress's
            ("screens", Some("Qpos"), ShowScene::ByName, true),
            ("screens_list", Some("gn"), ShowScene::ByName, true),
        ] {
            let player = load_swf(&game_with_scene_list(list, current, show, global));
            let mut player = player.lock().unwrap();

            let scenes = player
                .root_timeline()
                .and_then(|timeline| timeline.game_scenes)
                .unwrap_or_else(|| panic!("the movie should have a scene list in {list}"));
            assert_eq!(scenes.names, ["intro", "fight", "ending"]);
            let first = current.map(|_| 0);
            assert_eq!(scenes.current, first, "{list}, {current:?}");
            assert_eq!(scenes.previous(), None);
            assert_eq!(scenes.next(), Some(first.map_or(0, |first| first + 1)));

            player.goto_game_scene(2);
            let scenes = player.root_timeline().unwrap().game_scenes.unwrap();
            assert_eq!(scenes.current, Some(2), "{list}, {current:?}");
            assert_eq!(scenes.previous(), Some(1));
            assert_eq!(scenes.next(), None);

            player.goto_game_scene(0);
            let scenes = player.root_timeline().unwrap().game_scenes.unwrap();
            assert_eq!(scenes.current, Some(0), "{list}, {current:?}");
        }
    }

    #[test]
    fn navigate_scene_clips() {
        let player = load_swf(&game_with_scene_clips(&[
            "screen10",
            "screen2",
            "mainmenu",
            "screen_end",
            "trans_screen",
            "screen1",
            "moregames",
        ]));
        let mut player = player.lock().unwrap();

        let scenes = player
            .root_timeline()
            .and_then(|timeline| timeline.game_scenes)
            .expect("the movie's scenes should be its exported clips");
        assert_eq!(
            scenes.names,
            ["mainmenu", "screen1", "screen2", "screen10", "screen_end"]
        );
        assert_eq!(scenes.current, Some(0));

        player.goto_game_scene(3);
        let scenes = player.root_timeline().unwrap().game_scenes.unwrap();
        assert_eq!(scenes.current, Some(3));
        assert_eq!(scenes.next(), Some(4));

        // A list that a launcher left in _global, which names none of the clips, like One
        // Piece of Luck's, isn't the scenes'.
        let player = load_swf(&game_with_scene_clips_and_list(
            &["mainmenu", "screen1", "screen2"],
            "_global",
            "screens",
            &[string("screen_0"), string("screen_1")],
        ));
        let mut player = player.lock().unwrap();
        let scenes = player.root_timeline().unwrap().game_scenes.unwrap();
        assert_eq!(scenes.names, ["mainmenu", "screen1", "screen2"]);
        // Nor are numbers in a variable that other versions keep their lists in, like One
        // Piece Nami's.
        let player = load_swf(&game_with_scene_clips_and_list(
            &["main_menu", "screen_1", "screen_2", "end"],
            "_root",
            "Qvar",
            &[Value::Int(79615), Value::Int(85890), Value::Int(86476)],
        ));
        let mut player = player.lock().unwrap();
        let scenes = player.root_timeline().unwrap().game_scenes.unwrap();
        assert_eq!(scenes.names, ["main_menu", "screen_1", "screen_2"]);
        player.goto_game_scene(2);
        let scenes = player.root_timeline().unwrap().game_scenes.unwrap();
        assert_eq!(scenes.current, Some(2));
        // One that names them, like Big Queen 1's, is.
        let player = load_swf(&game_with_scene_clips_and_list(
            &["mainmenu", "screen_1", "screen_2"],
            "_global",
            "screens",
            &["screen_0", "screen_1", "screen_2", "screen_3"].map(string),
        ));
        let mut player = player.lock().unwrap();
        let scenes = player.root_timeline().unwrap().game_scenes.unwrap();
        assert_eq!(
            scenes.names,
            ["screen_0", "screen_1", "screen_2", "screen_3"]
        );

        // Names with underscores, like Universal Soldier's and Tifa's Happy Xmas's
        let player = load_swf(&game_with_scene_clips(&[
            "screen_failed",
            "screen_10",
            "screen_1",
            "trans_screen",
            "screen_2",
            "main_menu",
            "mainmenu_bg",
            "end",
        ]));
        let mut player = player.lock().unwrap();
        let scenes = player
            .root_timeline()
            .and_then(|timeline| timeline.game_scenes)
            .expect("the movie's scenes should be its exported clips");
        assert_eq!(
            scenes.names,
            [
                "main_menu",
                "screen_1",
                "screen_2",
                "screen_10",
                "screen_failed"
            ]
        );
    }

    /// A movie of old games' kind: one timeline of 8 frames, with no scenes or labels,
    /// whose code jumps its root timeline to frames 2, 4, 5 and 6 in the ways
    /// [`super::frame_jumps`] reads, and its movie clip to frame 7.
    fn timeline_game() -> Vec<u8> {
        use swf::{
            Button, ButtonAction, ButtonActionCondition, PlaceObject, PlaceObjectAction, Sprite,
            Tag,
        };

        // function next() { gotoAndPlay(2); } stop();
        let next = actions(&[Action::GotoFrame(swf::avm1::types::GotoFrame { frame: 1 })]);
        let mut root_code = actions(&[
            Action::DefineFunction(DefineFunction {
                name: "next".into(),
                params: vec![],
                actions: &next,
            }),
            Action::Stop,
        ]);
        root_code.push(0);
        // on (release) { gotoAndStop(4); }
        let mut button_code =
            actions(&[Action::GotoFrame(swf::avm1::types::GotoFrame { frame: 3 })]);
        button_code.push(0);
        // function jump() {
        //     _root.gotoAndPlay(5); tellTarget("_root") { gotoAndPlay(6); } gotoAndStop(7);
        // }
        let jump = actions(&[
            push(&[Value::Int(5), Value::Int(1), string("_root")]),
            Action::GetVariable,
            push(&[string("gotoAndPlay")]),
            Action::CallMethod,
            Action::Pop,
            Action::SetTarget(swf::avm1::types::SetTarget {
                target: "_root".into(),
            }),
            Action::GotoFrame(swf::avm1::types::GotoFrame { frame: 5 }),
            Action::SetTarget(swf::avm1::types::SetTarget { target: "".into() }),
            Action::GotoFrame(swf::avm1::types::GotoFrame { frame: 6 }),
        ]);
        let mut clip_code = actions(&[Action::DefineFunction(DefineFunction {
            name: "jump".into(),
            params: vec![],
            actions: &jump,
        })]);
        clip_code.push(0);

        let place = |id, depth| {
            Tag::PlaceObject(Box::new(PlaceObject {
                version: 2,
                action: PlaceObjectAction::Place(id),
                depth,
                matrix: None,
                color_transform: None,
                ratio: None,
                name: None,
                clip_depth: None,
                class_name: None,
                filters: None,
                background_color: None,
                blend_mode: None,
                clip_actions: None,
                has_image: false,
                is_bitmap_cached: None,
                is_visible: None,
                amf_data: None,
            }))
        };
        let mut tags = vec![
            Tag::DefineSprite(Sprite {
                id: 1,
                num_frames: 1,
                tags: vec![Tag::DoAction(&clip_code), Tag::ShowFrame],
            }),
            Tag::DefineButton2(Box::new(Button {
                id: 2,
                is_track_as_menu: false,
                records: vec![],
                actions: vec![ButtonAction {
                    conditions: ButtonActionCondition::OVER_DOWN_TO_OVER_UP,
                    action_data: &button_code,
                }],
            })),
            place(1, 1),
            place(2, 2),
            Tag::DoAction(&root_code),
        ];
        tags.extend((0..8).map(|_| Tag::ShowFrame));
        let header = swf::Header {
            num_frames: 8,
            ..swf::Header::default_with_swf_version(8)
        };
        let mut data = Vec::new();
        swf::write_swf(&header, &tags, &mut data).unwrap();
        data
    }

    #[test]
    fn navigate_frame_jumps() {
        let player = load_swf(&timeline_game());
        let mut player = player.lock().unwrap();

        let timeline = player.root_timeline().expect("root should be a movie clip");
        assert!(timeline.scenes_are_frame_jumps);
        assert!(timeline.game_scenes.is_none());
        let scenes: Vec<_> = timeline
            .scenes
            .iter()
            .map(|scene| (scene.name.as_str(), scene.start, scene.end()))
            .collect();
        assert_eq!(
            scenes,
            [
                ("Frame 1", 1, 1),
                ("Frame 2", 2, 3),
                ("Frame 4", 4, 4),
                ("Frame 5", 5, 5),
                ("Frame 6", 6, 8),
            ]
        );
        assert_eq!(timeline.current_scene(), Some(0));

        let next = timeline.next_scene().expect("Frame 2 follows Frame 1");
        player.goto_and_play_root_frame(next.start);
        let timeline = player.root_timeline().unwrap();
        assert_eq!(timeline.current_scene(), Some(1));
    }

    /// A movie whose clips show the version of a character that a button picks on frame 1,
    /// the ways games do: on frame 2, `hero` shows `_root.char`, which frame 1 sets, `hat`
    /// shows `hat_version`, which only the button sets (in `_global`), and `shirt` shows
    /// `_root.outfit`, which only a function sets, to what it's given. `face` shows frame 1
    /// if `_root.near1 == true`, and 2 if `near2` is, which frame 1 sets to false, like
    /// Gotham City's, and `mask` likewise with `mask1` and `mask2`, the second of which
    /// frame 1 sets to true.
    fn game_with_versions() -> Vec<u8> {
        use swf::avm1::types::{GotoFrame, GotoFrame2, If, StoreRegister};
        use swf::{
            Button, ButtonAction, ButtonActionCondition, PlaceObject, PlaceObjectAction, Sprite,
            Tag,
        };

        let show = |variable: &[Action<'static>]| {
            let mut code = actions(variable);
            code.extend(actions(&[Action::GotoFrame2(GotoFrame2 {
                set_playing: false,
                scene_offset: 0,
            })]));
            code.push(0);
            code
        };
        let root_member = |name: &'static str| {
            vec![
                push(&[string("_root")]),
                Action::GetVariable,
                push(&[string(name)]),
                Action::GetMember,
            ]
        };
        // gotoAndStop(_root.char), gotoAndStop(hat_version), gotoAndStop(_root.outfit)
        let hero = show(&root_member("char"));
        let hat = show(&[push(&[string("hat_version")]), Action::GetVariable]);
        let shirt = show(&root_member("outfit"));
        // if (_root.<first> == true) gotoAndStop(1); if (_root.<second> == true) gotoAndStop(2);
        let versions = |variables: [&'static str; 2]| {
            let mut code = Vec::new();
            for (frame, variable) in (0..).zip(variables) {
                let goto = actions(&[Action::GotoFrame(GotoFrame { frame })]);
                let mut condition = root_member(variable);
                condition.extend([
                    push(&[Value::Bool(true)]),
                    Action::Equals2,
                    Action::Not,
                    Action::If(If {
                        offset: goto.len() as i16,
                    }),
                ]);
                code.extend(actions(&condition));
                code.extend(goto);
            }
            code.push(0);
            code
        };
        let face = versions(["near1", "near2"]);
        let mask = versions(["mask1", "mask2"]);
        // on (release) { _root.char = 2; _global.hat_version = 3; gotoAndPlay(2);
        // gotoAndStop(NaN); }
        let mut button_code = actions(&[
            push(&[string("_root")]),
            Action::GetVariable,
            push(&[string("char"), Value::Int(2)]),
            Action::SetMember,
            push(&[string("_global")]),
            Action::GetVariable,
            push(&[string("hat_version"), Value::Int(3)]),
            Action::SetMember,
            Action::Play,
            push(&[string("NaN")]),
            Action::GetVariable,
            Action::GotoFrame2(GotoFrame2 {
                set_playing: false,
                scene_offset: 0,
            }),
        ]);
        button_code.push(0);
        // function pick(n) { _root.outfit = n; } char = 3; stop();
        let pick = actions(&[
            push(&[string("_root")]),
            Action::GetVariable,
            push(&[string("outfit"), string("n")]),
            Action::GetVariable,
            Action::StoreRegister(StoreRegister { register: 1 }),
            Action::SetMember,
        ]);
        let mut frame_1 = actions(&[
            Action::DefineFunction(DefineFunction {
                name: "pick".into(),
                params: vec!["n".into()],
                actions: &pick,
            }),
            push(&[string("char"), Value::Int(3)]),
            Action::SetVariable,
            push(&[string("near1"), Value::Bool(false)]),
            Action::SetVariable,
            push(&[string("near2"), Value::Bool(false)]),
            Action::SetVariable,
            push(&[string("mask2"), Value::Bool(true)]),
            Action::SetVariable,
            Action::Stop,
        ]);
        frame_1.push(0);
        let mut frame_2 = actions(&[Action::Stop]);
        frame_2.push(0);

        let place = |id, depth, name: Option<&'static str>| {
            Tag::PlaceObject(Box::new(PlaceObject {
                version: 2,
                action: PlaceObjectAction::Place(id),
                depth,
                matrix: None,
                color_transform: None,
                ratio: None,
                name: name.map(Into::into),
                clip_depth: None,
                class_name: None,
                filters: None,
                background_color: None,
                blend_mode: None,
                clip_actions: None,
                has_image: false,
                is_bitmap_cached: None,
                is_visible: None,
                amf_data: None,
            }))
        };
        let sprite = |id, code| {
            Tag::DefineSprite(Sprite {
                id,
                num_frames: 3,
                tags: vec![
                    Tag::DoAction(code),
                    Tag::ShowFrame,
                    Tag::ShowFrame,
                    Tag::ShowFrame,
                ],
            })
        };
        let tags = vec![
            sprite(1, &hero),
            sprite(2, &hat),
            sprite(3, &shirt),
            sprite(5, &face),
            sprite(6, &mask),
            Tag::DefineButton2(Box::new(Button {
                id: 4,
                is_track_as_menu: false,
                records: vec![],
                actions: vec![ButtonAction {
                    conditions: ButtonActionCondition::OVER_DOWN_TO_OVER_UP,
                    action_data: &button_code,
                }],
            })),
            place(4, 1, None),
            Tag::DoAction(&frame_1),
            Tag::ShowFrame,
            place(1, 2, Some("hero")),
            place(2, 3, Some("hat")),
            place(3, 4, Some("shirt")),
            place(5, 5, Some("face")),
            place(6, 6, Some("mask")),
            Tag::DoAction(&frame_2),
            Tag::ShowFrame,
        ];
        let header = swf::Header {
            num_frames: 2,
            ..swf::Header::default_with_swf_version(8)
        };
        let mut data = Vec::new();
        swf::write_swf(&header, &tags, &mut data).unwrap();
        data
    }

    /// The value of the root timeline's own variable `name`, as `Debug` writes it.
    fn root_variable(player: &mut Player, name: &str) -> String {
        use crate::display_object::TDisplayObject;
        player.mutate_with_update_context(|context| {
            let root = context.stage.root_clip().unwrap();
            let object = root.object1().unwrap();
            crate::avm1::Avm1::run_with_stack_frame_for_display_object(
                root,
                context,
                |activation| format!("{:?}", super::stored(object, name, activation)),
            )
        })
    }

    /// The frame that the movie clip named `name` on the root timeline is on.
    fn clip_frame(player: &mut Player, name: &str) -> Option<u16> {
        use crate::display_object::{TDisplayObject, TDisplayObjectContainer};
        player.mutate_with_update_context(|context| {
            let root = context.stage.root_clip()?.as_container()?;
            let clip =
                root.child_by_name(crate::string::WStr::from_units(name.as_bytes()), false)?;
            Some(clip.as_movie_clip()?.current_frame())
        })
    }

    #[test]
    fn find_frame_variables() {
        use super::{Constant, FrameVariable, Home, Versions, frame_variables};
        let variable = |home, name: &str, value| FrameVariable {
            home,
            name: name.to_owned(),
            value: Constant::Number(value),
        };
        let versions = |names: [&str; 2]| Versions {
            conditions: names
                .iter()
                .map(|name| (Home::Root, (*name).to_owned(), Constant::Bool(true)))
                .collect(),
            stops: false,
        };
        let picks = frame_variables(
            &swf::decompress_swf(&game_with_versions()[..]).unwrap().data,
            8,
        );
        assert_eq!(
            picks.variables,
            [
                variable(Home::Root, "char", 2.0),
                variable(Home::Global, "hat_version", 3.0),
                variable(Home::Root, "outfit", 1.0),
                variable(Home::Root, "NaN", 1.0),
            ]
        );
        assert_eq!(
            picks.versions,
            [versions(["near1", "near2"]), versions(["mask1", "mask2"])]
        );
    }

    #[test]
    fn fill_frame_variables_when_jumping() {
        let player = load_swf(&game_with_versions());
        let mut player = player.lock().unwrap();

        player.goto_and_play_root_frame(2);
        player.run_frame();
        player.run_frame();
        // The value that frame 1 gave char is kept, and the others get one. face shows its
        // first version, which none did, and mask the one that frame 1 picked.
        assert_eq!(clip_frame(&mut player, "hero"), Some(3));
        assert_eq!(clip_frame(&mut player, "hat"), Some(3));
        assert_eq!(clip_frame(&mut player, "shirt"), Some(1));
        assert_eq!(clip_frame(&mut player, "face"), Some(1));
        assert_eq!(clip_frame(&mut player, "mask"), Some(2));
        // NaN, which the code reads like a variable, is _global's, so the root doesn't get one.
        assert_eq!(root_variable(&mut player, "NaN"), "Undefined");
        // And they stay there.
        player.run_frame();
        for (clip, frame) in [
            ("hero", 3),
            ("hat", 3),
            ("shirt", 1),
            ("face", 1),
            ("mask", 2),
        ] {
            assert_eq!(
                clip_frame(&mut player, clip),
                Some(frame),
                "{clip} should stay"
            );
        }
    }

    /// A movie laid out like Metropolis Files' launcher, whose parts have reported their
    /// scenes: `files` holds two parts, with `screens` and a `clip` that keeps the name of
    /// the scene shown in `screen_name`, and `srv_send_screen(name)` shows a scene.
    fn game_with_scene_parts() -> Vec<u8> {
        // cur_file.clip.screen_name = name;
        let show_scene = actions(&[
            push(&[string("cur_file")]),
            Action::GetVariable,
            push(&[string("clip")]),
            Action::GetMember,
            push(&[string("screen_name"), string("name")]),
            Action::GetVariable,
            Action::SetMember,
        ]);
        // var <part> = new Object(); <part>.screens = [...]; <part>.clip = new Object();
        let part = |name: &'static str, screens: [&'static str; 2]| {
            [
                push(&[string(name), Value::Int(0), string("Object")]),
                Action::NewObject,
                Action::DefineLocal,
                push(&[string(name)]),
                Action::GetVariable,
                push(&[
                    string("screens"),
                    string(screens[1]),
                    string(screens[0]),
                    Value::Int(2),
                ]),
                Action::InitArray,
                Action::SetMember,
                push(&[string(name)]),
                Action::GetVariable,
                push(&[string("clip"), Value::Int(0), string("Object")]),
                Action::NewObject,
                Action::SetMember,
            ]
        };
        let mut setup = Vec::new();
        setup.extend(part("first", ["screen_1", "screen_0"]));
        setup.extend(part("second", ["screen_3", "screen_2"]));
        // var files = [first, second]; var cur_file = first; first.clip.screen_name = "screen_0";
        setup.extend([
            push(&[string("files"), string("second")]),
            Action::GetVariable,
            push(&[string("first")]),
            Action::GetVariable,
            push(&[Value::Int(2)]),
            Action::InitArray,
            Action::DefineLocal,
            push(&[string("cur_file"), string("first")]),
            Action::GetVariable,
            Action::DefineLocal,
            push(&[string("first")]),
            Action::GetVariable,
            push(&[string("clip")]),
            Action::GetMember,
            push(&[string("screen_name"), string("screen_0")]),
            Action::SetMember,
            Action::DefineFunction(DefineFunction {
                name: "srv_send_screen".into(),
                params: vec!["name".into()],
                actions: &show_scene,
            }),
        ]);
        let mut setup = actions(&setup);
        setup.push(0); // End
        movie(&[swf::Tag::DoAction(&setup), swf::Tag::ShowFrame])
    }

    #[test]
    fn navigate_scenes_of_parts() {
        let player = load_swf(&game_with_scene_parts());
        let mut player = player.lock().unwrap();

        let scenes = player
            .root_timeline()
            .and_then(|timeline| timeline.game_scenes)
            .expect("the movie's scenes should be the ones its parts reported");
        assert_eq!(
            scenes.names,
            ["screen_0", "screen_1", "screen_2", "screen_3"]
        );
        assert_eq!(scenes.current, Some(0));

        player.goto_game_scene(3);
        let scenes = player.root_timeline().unwrap().game_scenes.unwrap();
        assert_eq!(scenes.current, Some(3));
    }

    #[test]
    fn ignore_scene_lists_of_other_movies() {
        let player = load_swf(&game_with_scene_list(
            "scenelist",
            Some("pos"),
            ShowScene::Missing,
            false,
        ));
        let mut player = player.lock().unwrap();
        assert!(player.root_timeline().unwrap().game_scenes.is_none());

        // Without numbered scene clips, the exported clips aren't taken for scenes.
        let player = load_swf(&game_with_scene_clips(&["mainmenu", "screen_end"]));
        let mut player = player.lock().unwrap();
        assert!(player.root_timeline().unwrap().game_scenes.is_none());
    }
}
