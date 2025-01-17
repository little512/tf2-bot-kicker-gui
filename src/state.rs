use std::error::Error;

use crossbeam_channel::{Receiver, Sender};
use regex::Regex;

use crate::{
    command_manager::{self, CommandManager},
    logwatcher::LogWatcher,
    player_checker::{PlayerChecker, PLAYER_LIST, REGEX_LIST, PlayerRecord},
    regexes::{fn_lobby, fn_status, LogMatcher, REGEX_LOBBY, REGEX_STATUS},
    server::{Server, player::Team},
    settings::Settings,
    timer::Timer,
    version::VersionResponse, steamapi::{AccountInfoReceiver, self},
};

pub struct State {
    pub refresh_timer: Timer,
    pub alert_timer: Timer,
    pub kick_timer: Timer,

    pub settings: Settings,
    pub log: Option<LogWatcher>,

    pub server: Server,

    pub regx_status: LogMatcher,
    pub regx_lobby: LogMatcher,

    pub player_checker: PlayerChecker,

    pub latest_version: Option<Receiver<Result<VersionResponse, Box<dyn Error + Send>>>>,
    pub force_latest_version: bool,

    pub steamapi_request_sender: Sender<String>,
    pub steamapi_request_receiver: AccountInfoReceiver,

    demo_mode: bool,

    pub ui_context_menu_open: Option<usize>,
}

impl Default for State {
    fn default() -> Self {
        Self::new(false)
    }
}

impl State {
    pub fn new(demo_mode: bool) -> State {
        let settings: Settings;

        // Attempt to load settings, create new default settings if it can't load an existing file
        let set = Settings::import("cfg/settings.json");

        if let Ok(set) = set {
            settings = set;
        } else {
            settings = Settings::new();
            log::warn!(
                "{}",
                format!("Error loading settings: {}", set.unwrap_err())
            );
        }

        // Load regexes
        let regx_status = LogMatcher::new(Regex::new(REGEX_STATUS).unwrap(), fn_status);
        let regx_lobby = LogMatcher::new(Regex::new(REGEX_LOBBY).unwrap(), fn_lobby);

        // Create player checker and load any regexes and players saved
        let mut player_checker = PlayerChecker::new();
        match player_checker.read_players(PLAYER_LIST) {
            Ok(()) => {
                log::info!("Loaded playerlist");
            }
            Err(e) => {
                log::error!("Failed to read playlist: {:?}", e);
            }
        }
        match player_checker.read_regex_list(REGEX_LIST) {
            Ok(_) => {}
            Err(e) => {
                log::error!("{}", format!("Error loading {}: {}", REGEX_LIST, e));
            }
        }

        let log = LogWatcher::use_directory(&settings.tf2_directory);

        let (steamapi_request_sender, steamapi_request_receiver) = steamapi::create_api_thread(settings.steamapi_key.clone());

        let mut server = Server::new();

        // Add demo players to server
        if demo_mode {
            server.add_demo_player("Bash09".to_string(), "U:1:103663727".to_string(), Team::Invaders);
            server.add_demo_player("Baan".to_string(), "U:1:130631917".to_string(), Team::Defenders);
            server.add_demo_player("Random bot".to_string(), "U:1:1314494843".to_string(), Team::Defenders);
            server.add_demo_player("SmooveB".to_string(), "U:1:16722748".to_string(), Team::Invaders);
            server.add_demo_player("Some cunt".to_string(), "U:1:95849406".to_string(), Team::Invaders);
            server.add_demo_player("ASS".to_string(), "U:1:1203248403".to_string(), Team::Defenders);

            let mut records: Vec<PlayerRecord> = Vec::new();

            for p in server.get_players().values() {
                steamapi_request_sender.send(p.steamid64.clone()).ok();
                if let Some(record) = player_checker.check_player_steamid(&p.steamid32) {
                    records.push(record);
                }
            }

            for r in records {
                server.update_player_from_record(r);
            }
        }

        State {
            refresh_timer: Timer::new(),
            alert_timer: Timer::new(),
            kick_timer: Timer::new(),

            settings,
            log,
            server,

            regx_status,
            regx_lobby,

            player_checker,
            latest_version: None,
            force_latest_version: false,

            steamapi_request_sender,
            steamapi_request_receiver,

            demo_mode,

            ui_context_menu_open: None,
        }
    }

    pub fn is_demo(&self) -> bool {
        self.demo_mode
    }

    /// Begins a refresh on the local server state, any players unaccounted for since the last time this function was called will be removed.
    pub fn refresh(&mut self, cmd: &mut CommandManager) {
        if self.demo_mode {
            return;
        }

        if cmd.connected(&self.settings.rcon_password).is_err() {
            return;
        }
        self.server.prune();

        // Run status and tf_lobby_debug commands
        let status = cmd.run_command(command_manager::CMD_STATUS);
        let lobby = cmd.run_command(command_manager::CMD_TF_LOBBY_DEBUG);

        if status.is_none() || lobby.is_none() {
            return;
        }

        let lobby = lobby.unwrap();

        self.server.refresh();

        // Parse players from tf_lobby_debug output
        for l in lobby.lines() {
            match self.regx_lobby.r.captures(l) {
                None => {}
                Some(c) => {
                    (self.regx_lobby.f)(
                        &mut self.server,
                        l,
                        c,
                        &self.settings,
                        &mut self.player_checker,
                        cmd,
                    );
                }
            }
        }
    }
}
