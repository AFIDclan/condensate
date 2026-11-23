# Voice chat and playtest feedback

## Voice chat

Everyone in a multiplayer lobby can talk. Voice starts on its own when a game joins a lobby; games need no code for it.

- **Who handles the audio:** the Condensate app, not the game. It captures the microphone, removes background noise (RNNoise), detects speech and compresses it with Opus.
- **When it sends:** only while you talk, like Discord's voice activity. There's no push-to-talk.
- **How it travels:** frames go over the same server connection as the game's traffic, and the server forwards them to everyone else in the lobby who is on voice.
- **Settings:** in **Account → Voice chat**: microphone and speakers, automatic or manual sensitivity (with a level meter and a mic test), noise suppression and volume.
- **In a lobby:** a panel in the bottom-left corner of the app shows who's talking, with mute and deafen buttons.
- **Echo:** there's no echo cancellation yet. Use headphones, or friends will hear themselves through your microphone.

## Feedback recordings

The lobby's host can start a feedback recording from the voice panel in the app, or from the game with `Condensate.set_feedback(true)`. While it runs, the server saves:

- **Voice:** each player's voice, stored separately. The transcript always knows who said what.
- **Screenshots:** taken when a player presses **F12** in the game. They're saved with the game's version, the current scene, and anything the game adds through `Condensate.feedback_context`.

Players always know when they're being recorded:
- the game shows a "● REC" corner indicator and an announcement;
- the app's voice panel turns red;
- the Join button on the Friends page says "Join · REC".

Any player can leave their voice out with **Leave me out** in the voice panel.

When the recording stops, or the host leaves, the server transcribes each stretch of speech with Whisper. The game's owner then finds the recording in **Studio → Playtest feedback**. From there they can:
- read the timeline with the screenshots in place;
- save it to a folder;
- copy a command for an AI agent.

From the command line:

```sh
condensate feedback list my-game
condensate feedback pull my-game            # newest recording -> ./feedback/<date>-<id>/
condensate feedback pull my-game <id> --out somewhere
```

A pulled recording is a folder containing:
- `transcript.md`: a timeline of `[mm:ss] **Player:** what they said`, with each screenshot listed where it was taken.
- `shots/`: the screenshots.
- `feedback.json`: the same data, as JSON.

The Studio page's "Publish with an AI agent" instructions explain this to coding agents.

Only the game's owner (and server admins) can see recordings. Deleting a recording in Studio removes its audio and screenshots.

## Setting up transcription (once, on the server)

The server records without Whisper, but then the voice isn't transcribed. Recordings show as "Not transcribed" until Whisper is set up. To install it, run this on oort:

```sh
bash deploy/install-whisper.sh                 # large-v3-turbo-q5_0, about 550 MB
bash deploy/install-whisper.sh small.en        # faster on a CPU, less accurate
```

The script:
1. builds whisper.cpp in `~/.local/whisper`;
2. downloads the model;
3. sets `CONDENSATE_WHISPER` and `CONDENSATE_WHISPER_MODEL` in `~/condensate-server/condensate.env`;
4. restarts the service.

Recordings that were waiting are transcribed when the server starts. A recording that failed can be retried with **Transcribe again** in Studio.

Other settings:
- **`CONDENSATE_WHISPER_LANGUAGE`:** defaults to `auto`. Set `en` to skip language detection.
- **`CONDENSATE_FFMPEG`:** where to find ffmpeg, which converts the audio for Whisper.

oort has no GPU, so transcription runs on the CPU after the session, not live. A two-hour session usually has a lot less speech than that.

## How it fits together

- **Protocol** (`crates/core/src/play.rs`):
  - voice frames are binary mode 3;
  - screenshots are mode 4, sent from the game to the app;
  - the new commands are `voice`, `set_feedback` and `record_me`.
- **Server** (`crates/server/src/feedback.rs`):
  - the recorder writes each stretch of a player's speech to its own Ogg Opus file in `<data>/feedback/<id>/`;
  - the transcriber turns each file into 16 kHz WAV with ffmpeg and runs `whisper-cli` over a batch at a time;
  - routes are under `/api/v1/feedback`.
- **App:**
  - `client/src-tauri/src/voice.rs` handles the audio;
  - `game_api.rs` follows each game's lobby, carries voice and uploads screenshots over HTTP, so they don't hold up game traffic.
- **Godot addon** (`condensate.gd`): the F12 screenshot, the recording indicator, `set_feedback` and `feedback_context`.
- **Tests:** `bash scripts/play-test.sh` checks the relay, recording, opt-out, screenshots, the transcript and permissions.
  - With `APP=`, it also checks real audio through the app. A WAV of synthesized speech is fed in as the microphone.
  - With Whisper configured, it also checks the transcription.
