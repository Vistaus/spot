use gio::prelude::*;
use gio::{ActionMapExt, SimpleActionGroup};
use std::cell::Ref;
use std::ops::Deref;
use std::rc::Rc;

use crate::app::components::{
    handle_error, labels, PlaylistModel, SelectionTool, SelectionToolsModel,
};
use crate::app::dispatch::ActionDispatcher;
use crate::app::models::*;
use crate::app::state::{
    BrowserAction, BrowserEvent, PlaybackAction, PlaylistSource, SelectionAction, SelectionState,
};
use crate::app::{AppAction, AppEvent, AppModel, AppState, ListDiff};

pub struct DetailsModel {
    pub id: String,
    app_model: Rc<AppModel>,
    dispatcher: Box<dyn ActionDispatcher>,
}

impl DetailsModel {
    pub fn new(id: String, app_model: Rc<AppModel>, dispatcher: Box<dyn ActionDispatcher>) -> Self {
        Self {
            id,
            app_model,
            dispatcher,
        }
    }

    fn songs_ref(&self) -> Option<impl Deref<Target = Vec<SongDescription>> + '_> {
        self.app_model
            .map_state_opt(|s| Some(&s.browser.details_state(&self.id)?.content.as_ref()?.songs))
    }

    pub fn get_album_info(&self) -> Option<impl Deref<Target = AlbumDescription> + '_> {
        self.app_model
            .map_state_opt(|s| s.browser.details_state(&self.id)?.content.as_ref())
    }

    pub fn load_album_info(&self) {
        let id = self.id.clone();
        let api = self.app_model.get_spotify();
        self.dispatcher.dispatch_async(Box::pin(async move {
            match api.get_album(&id).await {
                Ok(album) => Some(BrowserAction::SetAlbumDetails(album).into()),
                Err(err) => handle_error(err),
            }
        }));
    }

    pub fn view_artist(&self) {
        if let Some(album) = self.get_album_info() {
            let artist = &album.artists.first().unwrap().id;
            self.dispatcher
                .dispatch(AppAction::ViewArtist(artist.to_owned()));
        }
    }

    pub fn toggle_save_album(&self) {
        if let Some(album) = self.get_album_info() {
            let id = album.id.clone();
            let is_liked = album.is_liked;

            let api = self.app_model.get_spotify();

            self.dispatcher.dispatch_async(Box::pin(async move {
                if !is_liked {
                    match api.save_album(&id).await {
                        Ok(album) => Some(BrowserAction::SaveAlbum(album).into()),
                        Err(err) => handle_error(err),
                    }
                } else {
                    match api.remove_saved_album(&id).await {
                        Ok(_) => Some(BrowserAction::UnsaveAlbum(id).into()),
                        Err(err) => handle_error(err),
                    }
                }
            }));
        }
    }
}

impl DetailsModel {
    fn state(&self) -> Ref<'_, AppState> {
        self.app_model.get_state()
    }
}

impl PlaylistModel for DetailsModel {
    fn select_song(&self, id: &str) {
        let song = self
            .songs_ref()
            .and_then(|songs| songs.iter().find(|&song| song.id == id).cloned());
        if let Some(song) = song {
            self.dispatcher
                .dispatch(SelectionAction::Select(vec![song]).into());
        }
    }

    fn deselect_song(&self, id: &str) {
        self.dispatcher
            .dispatch(SelectionAction::Deselect(vec![id.to_string()]).into());
    }

    fn enable_selection(&self) -> bool {
        self.dispatcher
            .dispatch(AppAction::ChangeSelectionMode(true));
        true
    }

    fn selection(&self) -> Option<Box<dyn Deref<Target = SelectionState> + '_>> {
        Some(Box::new(self.app_model.map_state(|s| &s.selection)))
    }

    fn current_song_id(&self) -> Option<String> {
        self.state().playback.current_song_id().cloned()
    }

    fn play_song(&self, id: &str) {
        let source = Some(PlaylistSource::Album(self.id.clone()));
        if self.app_model.get_state().playback.source() != source.as_ref() {
            let songs = self.songs_ref();
            if let Some(songs) = songs {
                self.dispatcher
                    .dispatch(PlaybackAction::LoadPlaylist(source, songs.clone()).into());
            }
        }
        self.dispatcher
            .dispatch(PlaybackAction::Load(id.to_string()).into());
    }

    fn diff_for_event(&self, event: &AppEvent) -> Option<ListDiff<SongModel>> {
        if matches!(
            event,
            AppEvent::BrowserEvent(BrowserEvent::AlbumDetailsLoaded(id)) if id == &self.id
        ) {
            let songs = self.songs_ref()?;
            Some(ListDiff::Set(
                songs
                    .iter()
                    .enumerate()
                    .map(|(i, s)| s.to_song_model(i))
                    .collect(),
            ))
        } else {
            None
        }
    }

    fn actions_for(&self, id: &str) -> Option<gio::ActionGroup> {
        let songs = self.songs_ref()?;
        let song = songs.iter().find(|&song| song.id == id)?;

        let group = SimpleActionGroup::new();

        for view_artist in song.make_artist_actions(self.dispatcher.box_clone(), None) {
            group.add_action(&view_artist);
        }
        group.add_action(&song.make_link_action(None));
        group.add_action(&song.make_queue_action(self.dispatcher.box_clone(), None));

        Some(group.upcast())
    }

    fn menu_for(&self, id: &str) -> Option<gio::MenuModel> {
        let songs = self.songs_ref()?;
        let song = songs.iter().find(|&song| song.id == id)?;

        let menu = gio::Menu::new();
        for artist in song.artists.iter() {
            menu.append(
                Some(&format!("{} {}", *labels::MORE_FROM, artist.name)),
                Some(&format!("song.view_artist_{}", artist.id)),
            );
        }

        menu.append(Some(&*labels::COPY_LINK), Some("song.copy_link"));
        menu.append(Some(&*labels::ADD_TO_QUEUE), Some("song.queue"));
        Some(menu.upcast())
    }
}

impl SelectionToolsModel for DetailsModel {
    fn selection(&self) -> Option<Box<dyn Deref<Target = SelectionState> + '_>> {
        Some(Box::new(self.app_model.map_state(|s| &s.selection)))
    }

    fn handle_tool_activated(&self, selection: &SelectionState, tool: &SelectionTool) {
        let action = match (tool, tool.default_action()) {
            (_, Some(action)) => Some(action),
            (SelectionTool::SelectAll, None) => self.songs_ref().map(|songs| {
                let songs = &*songs;
                let all_selected = selection.all_selected(songs.iter().map(|s| &s.id));

                if all_selected {
                    SelectionAction::Deselect(songs.iter().map(|s| &s.id).cloned().collect())
                } else {
                    SelectionAction::Select(songs.iter().cloned().collect())
                }
                .into()
            }),
            _ => None,
        };
        if let Some(action) = action {
            self.dispatcher.dispatch(action);
        }
    }
}
