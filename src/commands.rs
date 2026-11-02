//! Slash command definitions. Registered once in `ready` (globally, or in a
//! single guild when `GUILD_ID` is set for instant dev feedback).

use serenity::{builder::*, model::prelude::CommandOptionType};

fn opt(
    kind: CommandOptionType,
    name: &str,
    description: &str,
    required: bool,
) -> CreateCommandOption {
    CreateCommandOption::new(kind, name, description).required(required)
}

fn source_option() -> CreateCommandOption {
    opt(
        CommandOptionType::String,
        "source",
        "Where to look (default: both catalogues, searched in parallel)",
        false,
    )
    .add_string_choice("Auto · YouTube + SoundCloud", "auto")
    .add_string_choice("YouTube", "youtube")
    .add_string_choice("SoundCloud", "soundcloud")
}

pub fn all() -> Vec<CreateCommand> {
    vec![
        CreateCommand::new("play")
            .description("Play a song, a link or a whole playlist")
            .add_option(
                opt(
                    CommandOptionType::String,
                    "query",
                    "Song name, or any link supported by yt-dlp",
                    true,
                )
                .set_autocomplete(true),
            )
            .add_option(source_option())
            .add_option(opt(
                CommandOptionType::Boolean,
                "pick",
                "Show a result picker so you can choose the exact match",
                false,
            )),
        CreateCommand::new("search")
            .description("Find obscure tracks and pick the exact result")
            .add_option(
                opt(
                    CommandOptionType::String,
                    "query",
                    "Any song, artist, album or obscure upload",
                    true,
                )
                .set_autocomplete(true),
            )
            .add_option(source_option()),
        CreateCommand::new("panel").description("Post a fresh music control panel here"),
        CreateCommand::new("queue")
            .description("Browse the queue")
            .add_option(
                opt(CommandOptionType::Integer, "page", "Page number", false).min_int_value(1),
            ),
        CreateCommand::new("now").description("Show what's playing right now"),
        CreateCommand::new("pause").description("Pause or resume playback"),
        CreateCommand::new("skip")
            .description("Skip the current track (or jump to a queue position)")
            .add_option(
                opt(
                    CommandOptionType::Integer,
                    "to",
                    "Jump ahead to this queue position",
                    false,
                )
                .min_int_value(1),
            ),
        CreateCommand::new("stop").description("Stop playback and clear the queue"),
        CreateCommand::new("seek")
            .description("Seek the current track")
            .add_option(opt(
                CommandOptionType::String,
                "position",
                "e.g. 1:23, 01:02:03 or 90 (seconds)",
                true,
            )),
        CreateCommand::new("volume")
            .description("Set volume (0–200%)")
            .add_option(
                opt(
                    CommandOptionType::Integer,
                    "percent",
                    "0–200, where 100 is the untouched source volume",
                    true,
                )
                .min_int_value(0)
                .max_int_value(200),
            ),
        CreateCommand::new("loop")
            .description("Set the loop mode")
            .add_option(
                opt(
                    CommandOptionType::String,
                    "mode",
                    "Off, Track or Queue",
                    true,
                )
                .add_string_choice("Off", "off")
                .add_string_choice("Track", "track")
                .add_string_choice("Queue", "queue"),
            ),
        CreateCommand::new("shuffle").description("Shuffle the upcoming queue"),
        CreateCommand::new("remove")
            .description("Remove a track from the queue")
            .add_option(
                opt(
                    CommandOptionType::Integer,
                    "position",
                    "Queue position (1 = next)",
                    true,
                )
                .min_int_value(1),
            ),
        CreateCommand::new("join").description("Join your voice channel"),
        CreateCommand::new("leave").description("Leave voice and clear the session"),
        CreateCommand::new("stats").description("Runtime and audio information"),
        CreateCommand::new("help").description("List every command"),
    ]
}
