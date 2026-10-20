//! Snapshot of the root movie clip's timeline.
//!
//! Frontends use this to offer scene and frame label navigation, letting the
//! user jump anywhere on the root timeline without playing through the movie.

use crate::display_object::MovieClip;

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
        let data = std::fs::read(path).expect("test SWF should exist");
        let movie = SwfMovieData::from_data(&data, "file:///test.swf".into(), None, None)
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
    }
}
