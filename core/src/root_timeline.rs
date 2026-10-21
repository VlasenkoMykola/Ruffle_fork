//! Snapshot of the root movie clip's timeline.
//!
//! Frontends use this to offer scene and frame label navigation, letting the
//! user jump anywhere on the root timeline without playing through the movie.

use crate::avm1::{Activation, Avm1, Object as Avm1Object, Value};
use crate::context::UpdateContext;
use crate::display_object::{DisplayObject, MovieClip, TDisplayObject};
use crate::string::AvmString;

/// The variables of the root timeline in which games made with a certain engine keep
/// their own list of scenes, in each version of the engine.
///
/// Those games are a launcher movie that loads its scenes from other files, or keep
/// each scene in a movie clip, so the root timeline has no scenes of its own. They
/// switch scenes by calling the [`SHOW_SCENE`] function of the root timeline with a
/// name from the list.
const SCENE_LISTS: [&str; 2] = ["Qvar", "scenelist"];

/// The function of the root timeline that switches to the scene with the given name.
const SHOW_SCENE: &str = "setScreen";

/// The variable of the root timeline that holds the name of the scene being shown.
const CURRENT_SCENE_NAME: &str = "screen_name";

/// The variables of the root timeline that hold the 1-based number of the scene being
/// shown, in the versions of the engine that have one.
const CURRENT_SCENE_NUMBERS: [&str; 2] = ["Qpos", "pos"];

/// The variable of the root timeline that holds the number of scenes, in the version of
/// the engine whose [`SHOW_SCENE`] takes the 1-based number of a scene. The versions
/// without it take its name.
const SCENE_COUNT: &str = "Qfrm";

/// The most scenes read from a game's scene list.
const MAX_GAME_SCENES: i32 = 1000;

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

            let (list, length) = scene_list(object, activation)?;
            let names: Vec<_> = (0..length)
                .map(|index| match list.get_element(activation, index) {
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
            let (list, length) = scene_list(object, activation)?;
            let index = i32::try_from(index).ok().filter(|index| *index < length)?;
            if matches!(stored(object, SCENE_COUNT, activation), Value::Number(_)) {
                Some(Value::Number(f64::from(index) + 1.0))
            } else {
                Some(list.get_element(activation, index))
            }
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

/// The game's list of scenes in the root timeline `object`, and how many of them to read.
fn scene_list<'gc>(
    object: Avm1Object<'gc>,
    activation: &mut Activation<'_, 'gc>,
) -> Option<(Avm1Object<'gc>, i32)> {
    SCENE_LISTS
        .iter()
        .find_map(|name| match stored(object, name, activation) {
            Value::Object(list) => {
                let length = list.length(activation).ok().filter(|length| *length > 0)?;
                Some((list, length.min(MAX_GAME_SCENES)))
            }
            _ => None,
        })
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
        use swf::avm1::types::{Action, DefineFunction, Push, Value};

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
        let string = |string: &'static str| Value::Str(string.into());

        let show_scene = match show {
            // screen_name = name;
            ShowScene::ByName => actions(&[
                push(&[string("screen_name"), string("name")]),
                Action::GetVariable,
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
        if matches!(show, ShowScene::ByNumber) {
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

        let header = swf::Header {
            num_frames: 1,
            ..swf::Header::default_with_swf_version(8)
        };
        let tags = [swf::Tag::DoAction(&setup), swf::Tag::ShowFrame];
        let mut data = Vec::new();
        swf::write_swf(&header, &tags, &mut data).unwrap();
        data
    }

    #[test]
    fn navigate_game_scenes() {
        for (list, current, show) in [
            ("scenelist", Some("pos"), ShowScene::ByName),
            ("Qvar", Some("Qpos"), ShowScene::ByName),
            ("Qvar", None, ShowScene::ByNumber),
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
        }
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
    }
}
