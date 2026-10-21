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

/// The variables of the root timeline in which games made with a certain engine keep
/// their own list of scenes, in each version of the engine.
///
/// Those games are a launcher movie that loads its scenes from other files, or keep
/// each scene in a movie clip, so the root timeline has no scenes of its own. They
/// switch scenes by calling the [`SHOW_SCENE`] function of the root timeline with the
/// name of a scene from the list, or its number in the versions that count them.
const SCENE_LISTS: [&str; 2] = ["Qvar", "scenelist"];

/// The function of the root timeline that switches to the scene with the given name.
const SHOW_SCENE: &str = "setScreen";

/// The variable of the root timeline that holds the name of the scene being shown.
const CURRENT_SCENE_NAME: &str = "screen_name";

/// The variables of the root timeline that hold the 1-based number of the scene being
/// shown, in the versions of the engine that have one.
const CURRENT_SCENE_NUMBERS: [&str; 3] = ["Qpos", "pos", CURRENT_SCENE_TO_MOVE_FROM];

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

/// The export name of the movie clip of the main menu, in the version of the engine that
/// has no scene list: its [`SHOW_SCENE`] attaches the exported movie clip with the name
/// it's given, and the names of the others start with [`SCENE_CLIP_PREFIX`].
const MAIN_MENU_CLIP: &str = "mainmenu";

/// The start of the export names of the movie clips of scenes, in the version of the
/// engine that has no scene list: `screen1`, `screen2`..., and others like `screen_end`.
const SCENE_CLIP_PREFIX: &str = "screen";

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
    pub scenes: Vec<TimelineScene>,

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
            let show_scene = stored(object, SHOW_SCENE, activation);
            if !matches!(show_scene, Value::Object(show_scene) if show_scene.as_function().is_some())
            {
                return None;
            }

            let list = SceneList::of(root, object, activation)?;
            let names: Vec<_> = (0..list.len())
                .map(|index| match list.scene(index, activation) {
                    Value::String(name) => name.to_string(),
                    _ => String::new(),
                })
                .collect();

            let current = match stored(object, CURRENT_SCENE_NAME, activation) {
                Value::String(name) => {
                    let name = name.to_string();
                    names.iter().position(|scene| *scene == name)
                }
                _ => None,
            }
            .or_else(|| {
                CURRENT_SCENE_NUMBERS.iter().find_map(|variable| {
                    match stored(object, variable, activation) {
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
            if let SceneList::Clips(_) = list {
                return Some(list.scene(index, activation));
            }
            // Hitomi's version moves on from the current scene by the number it's given, so
            // it gets how far the scene is from that one.
            let current = stored(object, CURRENT_SCENE_TO_MOVE_FROM, activation);
            Some(match (current, stored(object, SCENE_COUNT, activation)) {
                (Value::Number(current), _) => Value::Number(number - current),
                (_, Value::Number(_)) => Value::Number(number),
                _ => list.scene(index, activation),
            })
        });
        let Some(scene) = scene else {
            return;
        };

        // The games' own controls stop the sounds of the scene they leave, and show the
        // mouse cursor, which some scenes hide.
        context.stop_all_sounds();
        context.ui.set_mouse_visible(true);
        let show_scene = AvmString::new_utf8(context.gc(), SHOW_SCENE);
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

/// The scenes of a game.
enum SceneList<'gc> {
    /// The array in one of the [`SCENE_LISTS`] variables, and how many of its scenes to read.
    Array(Avm1Object<'gc>, i32),

    /// The export names and IDs of the movie clips of the scenes, in the order the game
    /// numbers them, in the version of the engine that has no scene list.
    Clips(Vec<(AvmString<'gc>, CharacterId)>),
}

impl<'gc> SceneList<'gc> {
    /// The scenes of the game whose root timeline is `root`, with `object`, if it has some.
    fn of(
        root: DisplayObject<'gc>,
        object: Avm1Object<'gc>,
        activation: &mut Activation<'_, 'gc>,
    ) -> Option<Self> {
        let array = SCENE_LISTS
            .iter()
            .find_map(|name| match stored(object, name, activation) {
                Value::Object(list) => {
                    let length = list.length(activation).ok().filter(|length| *length > 0)?;
                    Some(Self::Array(list, length.min(MAX_GAME_SCENES)))
                }
                _ => None,
            });
        array.or_else(|| {
            let clips = scene_clips(root, activation.context);
            (!clips.is_empty()).then_some(Self::Clips(clips))
        })
    }

    fn len(&self) -> usize {
        match self {
            Self::Array(_, length) => *length as usize,
            Self::Clips(clips) => clips.len(),
        }
    }

    /// The scene at `index` (less than [`Self::len`]), as it's listed.
    fn scene(&self, index: usize, activation: &mut Activation<'_, 'gc>) -> Value<'gc> {
        match self {
            Self::Array(list, _) => list.get_element(activation, index as i32),
            Self::Clips(clips) => Value::String(clips[index].0),
        }
    }
}

/// The export names and IDs of the movie clips that the root timeline `root` exports for
/// its scenes, in the version of the engine that has no scene list: the main menu, then
/// `screen1`, `screen2`..., then the others whose names start with `screen_`. There are
/// none unless numbered ones are there.
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
            let order = if lower == MAIN_MENU_CLIP {
                (0, 0)
            } else {
                let rest = lower.strip_prefix(SCENE_CLIP_PREFIX)?;
                match rest.parse::<u32>() {
                    Ok(number) if rest.bytes().all(|byte| byte.is_ascii_digit()) => (1, number),
                    _ if rest.starts_with('_') => (2, 0),
                    _ => return None,
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
            labels,
            current_frame: mc.current_frame(),
            total_frames,
            frames_loaded,
            game_scenes: None,
        }
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
        let mut setup = vec![
            push(&[
                string(list),
                string("ending"),
                string("fight"),
                string("intro"),
                Value::Int(3),
            ]),
            Action::InitArray,
            Action::DefineLocal,
            push(&[string("screen_name")]),
            Action::DefineLocal2,
        ];
        if let Some(current) = current {
            setup.extend([push(&[string(current), Value::Int(1)]), Action::DefineLocal]);
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
        // attachMovie(name, "screen", 0);
        let show_scene = actions(&[
            push(&[Value::Int(0), string("screen"), string("name")]),
            Action::GetVariable,
            push(&[Value::Int(3), string("attachMovie")]),
            Action::CallFunction,
            Action::Pop,
        ]);
        // function setScreen(name) { ... } setScreen("mainmenu");
        let mut setup = actions(&[
            Action::DefineFunction(DefineFunction {
                name: "setScreen".into(),
                params: vec!["name".into()],
                actions: &show_scene,
            }),
            push(&[string("mainmenu"), Value::Int(1), string("setScreen")]),
            Action::CallFunction,
            Action::Pop,
        ]);
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
        for (list, current, show) in [
            ("scenelist", Some("pos"), ShowScene::ByName),
            ("Qvar", Some("Qpos"), ShowScene::ByName),
            ("Qvar", None, ShowScene::ByNumber),
            ("Qvar", Some("Qmod"), ShowScene::ByStep),
        ] {
            let player = load_swf(&game_with_scene_list(list, current, show));
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
    }

    #[test]
    fn ignore_scene_lists_of_other_movies() {
        let player = load_swf(&game_with_scene_list(
            "scenelist",
            Some("pos"),
            ShowScene::Missing,
        ));
        let mut player = player.lock().unwrap();
        assert!(player.root_timeline().unwrap().game_scenes.is_none());

        // Without numbered scene clips, the exported clips aren't taken for scenes.
        let player = load_swf(&game_with_scene_clips(&["mainmenu", "screen_end"]));
        let mut player = player.lock().unwrap();
        assert!(player.root_timeline().unwrap().game_scenes.is_none());
    }
}
