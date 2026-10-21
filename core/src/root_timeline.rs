//! Snapshot of the root movie clip's timeline.
//!
//! Frontends use this to offer scene and frame label navigation, letting the
//! user jump anywhere on the root timeline without playing through the movie.

use crate::avm1::{Avm1, Value};
use crate::context::UpdateContext;
use crate::display_object::{DisplayObject, MovieClip, TDisplayObject};
use crate::string::AvmString;

/// The variable of the root timeline that holds the 1-based number of the scene a game
/// should switch to, in games that keep their own list of scenes.
///
/// Some games keep their scenes this way, in a launcher movie that loads the
/// scenes from other files, so the root timeline has none of its own. They switch
/// scenes with keys like Page Up and Page Down by setting this variable.
const REQUESTED_SCENE: &str = "Qscr";

/// The variables of the root timeline that hold the list of scenes and the 1-based
/// number of the scene being shown, in each version of the engine of those games.
const SCENE_VARIABLES: [(&str, &str); 2] = [("Qvar", "Qpos"), ("scenelist", "pos")];

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

/// The scenes of a game that keeps its own list of them in variables of its
/// root timeline, which [`crate::Player::goto_game_scene`] switches between.
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
            // The games declare this variable up front, which other movies are unlikely to.
            let requested = AvmString::new_utf8(activation.gc(), REQUESTED_SCENE);
            if !object.has_own_property(activation, requested) {
                return None;
            }

            SCENE_VARIABLES
                .iter()
                .find_map(|(list_name, current_name)| {
                    let name = AvmString::new_utf8(activation.gc(), *list_name);
                    let Ok(Value::Object(list)) = object.get_stored(name, activation) else {
                        return None;
                    };
                    let length = list.length(activation).ok().filter(|length| *length > 0)?;
                    let names: Vec<_> = (0..length.min(MAX_GAME_SCENES))
                        .map(|index| match list.get_element(activation, index) {
                            Value::String(name) => name.to_string(),
                            _ => String::new(),
                        })
                        .collect();

                    let name = AvmString::new_utf8(activation.gc(), *current_name);
                    let current = match object.get_stored(name, activation) {
                        Ok(Value::Number(number))
                            if number >= 1.0 && number <= names.len() as f64 =>
                        {
                            Some(number as usize - 1)
                        }
                        _ => None,
                    };
                    Some(Self { names, current })
                })
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

    /// Asks the game whose root timeline is `root` to switch to the scene at `index`
    /// in its scene list, which it does when it next checks.
    pub(crate) fn request<'gc>(
        context: &mut UpdateContext<'gc>,
        root: DisplayObject<'gc>,
        index: usize,
    ) {
        let Some(object) = root.object1() else {
            return;
        };
        Avm1::run_with_stack_frame_for_display_object(root, context, |activation| {
            let name = AvmString::new_utf8(activation.gc(), REQUESTED_SCENE);
            if let Err(e) = object.set(name, Value::Number(index as f64 + 1.0), activation) {
                tracing::warn!("Couldn't request game scene {index}: {e}");
            }
        });
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

    /// A movie that keeps its scene list like the games [`super::GameScenes`] is for, in
    /// the variables `list` and `current`, and shows the scene requested in `Qscr` on its
    /// second frame. Without `declares_request`, it doesn't declare `Qscr` up front.
    fn game_with_scene_list(
        list: &'static str,
        current: &'static str,
        declares_request: bool,
    ) -> Vec<u8> {
        use swf::avm1::types::{Action, Push, Value};

        fn code(actions: &[Action<'_>]) -> Vec<u8> {
            let mut code = Vec::new();
            let mut writer = swf::avm1::write::Writer::new(&mut code, 8);
            for action in actions {
                writer.write_action(action).unwrap();
            }
            code.push(0); // End
            code
        }
        fn push(values: &[Value<'static>]) -> Action<'static> {
            Action::Push(Push {
                values: values.to_vec(),
            })
        }
        let string = |string: &'static str| Value::Str(string.into());

        // var <list> = ["intro", "fight", "ending"]; var <current> = 1; var Qscr;
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
            push(&[string(current), Value::Int(1)]),
            Action::DefineLocal,
        ];
        if declares_request {
            setup.extend([push(&[string("Qscr")]), Action::DefineLocal2]);
        }
        let setup = code(&setup);
        // <current> = Qscr; stop();
        let show_requested = code(&[
            push(&[string(current), string("Qscr")]),
            Action::GetVariable,
            Action::SetVariable,
            Action::Stop,
        ]);

        let header = swf::Header {
            num_frames: 2,
            ..swf::Header::default_with_swf_version(8)
        };
        let tags = [
            swf::Tag::DoAction(&setup),
            swf::Tag::ShowFrame,
            swf::Tag::DoAction(&show_requested),
            swf::Tag::ShowFrame,
        ];
        let mut data = Vec::new();
        swf::write_swf(&header, &tags, &mut data).unwrap();
        data
    }

    #[test]
    fn navigate_game_scenes() {
        for (list, current) in [("scenelist", "pos"), ("Qvar", "Qpos")] {
            let player = load_swf(&game_with_scene_list(list, current, true));
            let mut player = player.lock().unwrap();

            let scenes = player
                .root_timeline()
                .and_then(|timeline| timeline.game_scenes)
                .unwrap_or_else(|| panic!("the movie should have a scene list in {list}"));
            assert_eq!(scenes.names, ["intro", "fight", "ending"]);
            assert_eq!(scenes.current, Some(0));
            assert_eq!(scenes.previous(), None);
            assert_eq!(scenes.next(), Some(1));

            player.goto_game_scene(2);
            player.run_frame();
            let scenes = player.root_timeline().unwrap().game_scenes.unwrap();
            assert_eq!(scenes.current, Some(2));
            assert_eq!(scenes.previous(), Some(1));
            assert_eq!(scenes.next(), None);
        }
    }

    #[test]
    fn ignore_scene_lists_of_other_movies() {
        let player = load_swf(&game_with_scene_list("scenelist", "pos", false));
        let mut player = player.lock().unwrap();
        assert!(player.root_timeline().unwrap().game_scenes.is_none());
    }
}
