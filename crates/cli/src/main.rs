mod auth;
mod config;
mod setup;

use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use condensate_core::api::*;
use condensate_core::{platform, LaunchConfig};
use condensate_sdk::pull::{PullEvent, PullOptions};
use condensate_sdk::push::{PushBuild, PushEvent, PushOptions};
use condensate_sdk::Client;

use condensate_sdk::login_store::{self, StoredLogin};

use crate::config::ProjectFile;

/// Condensate: share games with friends.
///
/// Uses the same login as the Condensate desktop app. If you're not logged in, it opens the
/// app so you can log in there. Build agents should set CONDENSATE_TOKEN (a publish token)
/// and CONDENSATE_SERVER instead.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Server URL, e.g. https://oort-cloud.example.ts.net
    #[arg(long, global = true, env = "CONDENSATE_SERVER")]
    server: Option<String>,

    /// API token (overrides the saved login)
    #[arg(long, global = true, env = "CONDENSATE_TOKEN", hide_env_values = true)]
    token: Option<String>,

    /// Print results as JSON
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create an account with an invite code (the first account on a server becomes admin)
    Register {
        username: String,
        #[arg(long)]
        invite: Option<String>,
        #[arg(long)]
        display_name: Option<String>,
    },
    /// Log in (also logs in the desktop app). Password from CONDENSATE_PASSWORD or a prompt.
    Login { username: String },
    /// Log out (also logs out the desktop app)
    Logout,
    /// Show who you are logged in as
    Whoami,
    /// Invite codes for friends
    #[command(subcommand)]
    Invite(InviteCmd),
    /// Publish tokens for build agents
    #[command(subcommand)]
    Token(TokenCmd),
    /// Create, inspect and search games
    #[command(subcommand)]
    Game(GameCmd),
    /// Upload a new version of a game. Reads ./condensate.toml unless --dir/--exe are given.
    Push(PushArgs),
    /// Install a game into a folder, or update the copy already there
    Pull {
        game: String,
        dir: PathBuf,
        #[arg(long)]
        platform: Option<String>,
        /// Re-check every file on disk and fix anything damaged
        #[arg(long)]
        repair: bool,
    },
    /// Start the game installed in a folder
    Launch { dir: PathBuf },
    /// Show a game's version history and changelogs
    Versions { game: String },
    /// Playtest feedback recordings: transcribed voice chat and players' screenshots
    #[command(subcommand)]
    Feedback(FeedbackCmd),
    /// Installer support: add or remove the CLI from your PATH
    #[command(hide = true, subcommand)]
    Setup(SetupCmd),
}

#[derive(Subcommand)]
enum FeedbackCmd {
    /// List a game's feedback recordings, newest first
    List { game: String },
    /// Save a recording into a folder: transcript.md (who said what, with screenshots in
    /// place), feedback.json and shots/
    Pull {
        game: String,
        /// Recording id (default: the newest)
        id: Option<String>,
        /// Folder to save into
        #[arg(long, default_value = "feedback")]
        out: PathBuf,
    },
}

#[derive(Subcommand)]
enum SetupCmd {
    AddToPath,
    RemoveFromPath,
}

#[derive(Subcommand)]
enum InviteCmd {
    /// Create a single-use invite code
    Create,
    /// List invite codes you created
    List,
}

#[derive(Subcommand)]
enum TokenCmd {
    /// Create a publish token (shown once)
    Create {
        #[arg(long)]
        name: String,
        /// Only allow publishing this game
        #[arg(long)]
        game: Option<String>,
    },
    /// List your tokens and sessions
    List,
    /// Revoke a token by id
    Revoke { id: i64 },
}

#[derive(Subcommand)]
enum GameCmd {
    /// Register a new game id (it appears in the store once a version is pushed)
    Create {
        slug: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        summary: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// public, unlisted or private
        #[arg(long)]
        visibility: Option<String>,
    },
    /// Edit a game's store page and artwork
    Edit {
        slug: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        summary: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        visibility: Option<String>,
        /// Poster image (portrait, 2:3, e.g. 600x900 PNG/JPEG)
        #[arg(long)]
        cover: Option<PathBuf>,
        /// Banner image (landscape, 16:9, e.g. 1920x1080 PNG/JPEG)
        #[arg(long)]
        banner: Option<PathBuf>,
    },
    /// Delete one version of a game
    DeleteVersion { slug: String, version: String },
    /// Games you own, including unreleased ones
    Mine,
    /// Show one game
    Info { slug: String },
    /// Search the store
    Search { query: Option<String> },
    /// Your library
    Library,
    /// Add a game to your library
    Add { slug: String },
}

#[derive(Args)]
struct PushArgs {
    /// What changed in this version (required, keep it short)
    #[arg(long, short = 'm')]
    changelog: String,
    /// Version number; defaults to bumping the last one (0.4.2 -> 0.4.3)
    #[arg(long)]
    version: Option<String>,
    /// Project file to read
    #[arg(long, default_value = "condensate.toml")]
    config: PathBuf,
    /// Push a single build folder without a project file (needs --game, --platform, --exe)
    #[arg(long)]
    dir: Option<PathBuf>,
    #[arg(long)]
    game: Option<String>,
    #[arg(long)]
    platform: Option<String>,
    /// Executable path relative to --dir
    #[arg(long)]
    exe: Option<String>,
    /// Only push these platforms from the project file
    #[arg(long = "only")]
    only: Vec<String>,
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

struct Ctx {
    login: StoredLogin,
    server_override: Option<String>,
    token_override: Option<String>,
    json: bool,
}

impl Ctx {
    fn server(&self, project: Option<&str>) -> Option<String> {
        self.server_override.clone().or_else(|| project.map(String::from)).or_else(|| self.login.server.clone())
    }

    async fn client(&self) -> Result<Client> {
        self.client_for(None).await
    }

    /// An authenticated client: explicit token, else the shared app/CLI login, else (when a
    /// person is at the keyboard) open the app and wait for them to log in.
    async fn client_for(&self, project_server: Option<&str>) -> Result<Client> {
        let wanted = self.server_override.clone().or_else(|| project_server.map(String::from));
        if let Some(token) = &self.token_override {
            let server = self.server(project_server).ok_or_else(|| anyhow!("CONDENSATE_TOKEN is set but no server: set CONDENSATE_SERVER"))?;
            return Client::new(&server, Some(token.clone()));
        }
        let usable = |l: &StoredLogin| match (&l.token, &l.server) {
            (Some(t), Some(s)) if wanted.as_deref().map_or(true, |w| auth::same_server(w, s)) => Some((s.clone(), t.clone())),
            _ => None,
        };
        if let Some((server, token)) = usable(&self.login) {
            return Client::new(&server, Some(token));
        }
        if self.login.token.is_some() {
            bail!(
                "you're logged in to {}, not {}; run `condensate login <username> --server {}`",
                self.login.server.as_deref().unwrap_or("?"),
                wanted.as_deref().unwrap_or("?"),
                wanted.as_deref().unwrap_or("?")
            );
        }
        if std::io::stdin().is_terminal() && std::io::stderr().is_terminal() {
            if let Some(login) = auth::login_with_app().await? {
                if let Some((server, token)) = usable(&login) {
                    return Client::new(&server, Some(token));
                }
                bail!("the app logged in to {}, but this command wants {}", login.server.unwrap_or_default(), wanted.unwrap_or_default());
            }
        }
        bail!("not logged in: log in to the Condensate app, run `condensate login <username>`, or set CONDENSATE_TOKEN")
    }

    fn anon_client(&self) -> Result<Client> {
        let server = self.server(None).ok_or_else(|| anyhow!("which server? pass --server https://..."))?;
        Client::new(&server, None)
    }

    fn print<T: serde::Serialize>(&self, value: &T, human: impl FnOnce()) {
        if self.json {
            println!("{}", serde_json::to_string_pretty(value).unwrap());
        } else {
            human();
        }
    }
}

fn password(prompt: &str) -> Result<String> {
    if let Ok(p) = std::env::var("CONDENSATE_PASSWORD") {
        return Ok(p);
    }
    if !std::io::stdin().is_terminal() {
        bail!("set CONDENSATE_PASSWORD to log in non-interactively");
    }
    Ok(rpassword::prompt_password(prompt)?)
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let ctx = Ctx { login: auth::current_login(), server_override: cli.server, token_override: cli.token, json: cli.json };

    match cli.command {
        Command::Register { username, invite, display_name } => {
            let client = ctx.anon_client()?;
            let pw = password("Choose a password: ")?;
            let resp = client
                .register(&RegisterRequest { username, password: pw, display_name, invite_code: invite })
                .await?;
            save_session(&client, &resp)?;
            ctx.print(&resp.user, || println!("Registered and logged in as {}", resp.user.username));
        }
        Command::Login { username } => {
            let client = ctx.anon_client()?;
            let pw = password("Password: ")?;
            let resp = client.login(&LoginRequest { username, password: pw }).await?;
            save_session(&client, &resp)?;
            ctx.print(&resp.user, || println!("Logged in as {}", resp.user.username));
        }
        Command::Logout => {
            if let (Some(server), Some(token)) = (&ctx.login.server, &ctx.login.token) {
                let _ = Client::new(server, Some(token.clone()))?.logout().await;
            }
            login_store::save(&StoredLogin { token: None, ..ctx.login.clone() })?;
            auth::clear_legacy();
            println!("Logged out (here and in the desktop app)");
        }
        Command::Whoami => {
            let client = ctx.client().await?;
            let me = client.me().await?;
            ctx.print(&me, || println!("{} ({}) on {}", me.username, me.display_name, client.server()));
        }
        Command::Invite(InviteCmd::Create) => {
            let invite = ctx.client().await?.create_invite().await?;
            ctx.print(&invite, || println!("{}", invite.code));
        }
        Command::Invite(InviteCmd::List) => {
            let invites = ctx.client().await?.list_invites().await?;
            ctx.print(&invites, || {
                for i in &invites {
                    match &i.used_by {
                        Some(u) => println!("{}  used by {u}", i.code),
                        None => println!("{}  unused", i.code),
                    }
                }
            });
        }
        Command::Token(TokenCmd::Create { name, game }) => {
            let created = ctx.client().await?.create_token(&CreateTokenRequest { name, game }).await?;
            ctx.print(&created, || {
                println!("{}", created.token);
                eprintln!("Save this now; it is not shown again. Use it as CONDENSATE_TOKEN.");
            });
        }
        Command::Token(TokenCmd::List) => {
            let tokens = ctx.client().await?.list_tokens().await?;
            ctx.print(&tokens, || {
                for t in &tokens {
                    let scope = t.game.as_deref().map(|g| format!(" game={g}")).unwrap_or_default();
                    println!("{:>5}  {:<8} {}{}", t.id, t.kind, t.name, scope);
                }
            });
        }
        Command::Token(TokenCmd::Revoke { id }) => {
            ctx.client().await?.revoke_token(id).await?;
            println!("Revoked token {id}");
        }
        Command::Game(cmd) => game_cmd(&ctx, cmd).await?,
        Command::Feedback(FeedbackCmd::List { game }) => {
            let list = ctx.client().await?.feedback_list(&game).await?;
            ctx.print(&list, || {
                if list.is_empty() {
                    println!("No feedback recordings for {game} yet. The host of a lobby can start one in the Condensate app.");
                }
                for f in &list {
                    let (y, mo, d, h, mi) = condensate_core::utc_parts(f.started_at);
                    println!(
                        "{}  {y}-{mo:02}-{d:02} {h:02}:{mi:02} UTC  {:<13}  {} ({} lines, {} screenshots)",
                        f.id,
                        f.status,
                        f.players.join(", "),
                        f.utterances,
                        f.shots
                    );
                }
            });
        }
        Command::Feedback(FeedbackCmd::Pull { game, id, out }) => {
            let client = ctx.client().await?;
            let list = client.feedback_list(&game).await?;
            let summary = match &id {
                Some(id) => list.iter().find(|f| f.id == *id).ok_or_else(|| anyhow!("{game} has no feedback recording {id}"))?,
                None => list.first().ok_or_else(|| anyhow!("{game} has no feedback recordings yet"))?,
            };
            let dir = condensate_sdk::feedback::export(&client, &summary.id, &out).await?;
            match summary.status.as_str() {
                "ready" => {}
                "recording" => eprintln!("Note: this recording is still going; pull again when it's done."),
                "transcribing" => eprintln!("Note: transcription isn't finished; pull again later for the full text."),
                _ => eprintln!("Note: {}", summary.note.as_deref().unwrap_or("the voice wasn't transcribed")),
            }
            println!("{}", dir.join("transcript.md").display());
        }
        Command::Push(args) => push_cmd(&ctx, args).await?,
        Command::Pull { game, dir, platform, repair } => {
            let platform = platform
                .or_else(|| platform::current().map(String::from))
                .ok_or_else(|| anyhow!("unsupported platform; pass --platform"))?;
            let client = ctx.client().await?;
            let quiet = ctx.json;
            let report = condensate_sdk::pull::pull(
                &client,
                PullOptions { game, platform, dir, repair },
                &move |e| if !quiet { pull_progress(e) },
            )
            .await?;
            ctx.print(&report, || {
                if report.up_to_date {
                    println!("{} {} is up to date", report.game, report.to_version);
                } else {
                    let from = report.from_version.as_deref().map(|v| format!("{v} -> ")).unwrap_or_default();
                    println!(
                        "{} {from}{}: downloaded {}, reused {} from disk, {} files written, {} removed",
                        report.game,
                        report.to_version,
                        human_bytes(report.downloaded_bytes),
                        human_bytes(report.reused_bytes),
                        report.files_written,
                        report.files_deleted
                    );
                    println!("Changelog: {}", report.changelog);
                }
            });
        }
        Command::Launch { dir } => {
            // Let the game reach the multiplayer API when the app is running.
            let game = condensate_core::install::read_installed(&dir)?.map(|s| s.game);
            let env = match (condensate_sdk::login_store::game_api(), game) {
                (Some((api, token)), Some(game)) => vec![("CONDENSATE_API", api), ("CONDENSATE_TOKEN", token), ("CONDENSATE_GAME", game)],
                _ => vec![],
            };
            let mut child = condensate_sdk::launch::launch(&dir, &env)?;
            let status = child.wait()?;
            if !status.success() {
                bail!("game exited with {status}");
            }
        }
        Command::Setup(SetupCmd::AddToPath) => setup::add_to_path()?,
        Command::Setup(SetupCmd::RemoveFromPath) => setup::remove_from_path()?,
        Command::Versions { game } => {
            let versions = ctx.client().await?.versions(&game).await?;
            ctx.print(&versions, || {
                for v in &versions {
                    println!("{}  [{}]  {}", v.version, v.platforms.join(", "), v.changelog);
                }
            });
        }
    }
    Ok(())
}

fn save_session(client: &Client, resp: &AuthResponse) -> Result<()> {
    auth::clear_legacy();
    login_store::save(&StoredLogin {
        server: Some(client.server().to_string()),
        token: Some(resp.token.clone()),
        username: Some(resp.user.username.clone()),
    })
}

async fn game_cmd(ctx: &Ctx, cmd: GameCmd) -> Result<()> {
    let client = ctx.client().await?;
    match cmd {
        GameCmd::Create { slug, title, summary, description, visibility } => {
            let game = client.create_game(&CreateGameRequest { slug, title, summary, description, visibility }).await?;
            ctx.print(&game, || println!("Created {} ({})", game.summary.title, game.summary.slug));
        }
        GameCmd::Edit { slug, title, summary, description, visibility, cover, banner } => {
            let cover_image = match cover {
                Some(p) => Some(upload_image(&client, &p).await?),
                None => None,
            };
            let banner_image = match banner {
                Some(p) => Some(upload_image(&client, &p).await?),
                None => None,
            };
            let req = UpdateGameRequest { title, summary, description, visibility, cover_image, banner_image };
            let game = client.update_game(&slug, &req).await?;
            ctx.print(&game, || println!("Updated {}", game.summary.slug));
        }
        GameCmd::DeleteVersion { slug, version } => {
            client.delete_version(&slug, &version).await?;
            println!("Deleted {slug} {version}");
        }
        GameCmd::Mine => {
            let games = client.my_games().await?;
            ctx.print(&games, || print_games(&games));
        }
        GameCmd::Info { slug } => {
            let game = client.game(&slug).await?;
            ctx.print(&game, || {
                let s = &game.summary;
                println!("{} ({}) by {}", s.title, s.slug, s.owner);
                if !s.summary.is_empty() {
                    println!("{}", s.summary);
                }
                println!(
                    "Latest: {}  Platforms: {}",
                    s.latest_version.as_deref().unwrap_or("none"),
                    if s.platforms.is_empty() { "none".into() } else { s.platforms.join(", ") }
                );
                for v in game.versions.iter().take(5) {
                    println!("  {}  {}", v.version, v.changelog);
                }
            });
        }
        GameCmd::Search { query } => {
            let games = client.search_games(query.as_deref()).await?;
            ctx.print(&games, || print_games(&games));
        }
        GameCmd::Library => {
            let entries = client.library(platform::current()).await?;
            ctx.print(&entries, || {
                for e in &entries {
                    let latest = e.latest.as_ref().map(|l| l.version.as_str()).unwrap_or("-");
                    println!("{:<24} {:<10} {}", e.game.slug, latest, e.game.title);
                }
            });
        }
        GameCmd::Add { slug } => {
            client.library_add(&slug).await?;
            println!("Added {slug} to your library");
        }
    }
    Ok(())
}

async fn upload_image(client: &Client, path: &std::path::Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    client.upload_image(bytes).await.with_context(|| format!("uploading {}", path.display()))
}

/// Upload the project's artwork and point the game at it if it changed.
async fn sync_art(client: &Client, game: &str, art: &config::ProjectArt) -> Result<()> {
    if art.cover.is_none() && art.banner.is_none() {
        return Ok(());
    }
    let current = client.game(game).await?.summary;
    let mut req = UpdateGameRequest::default();
    if let Some(path) = &art.cover {
        let id = upload_image(client, path).await?;
        if current.cover.as_deref() != Some(id.as_str()) {
            req.cover_image = Some(id);
        }
    }
    if let Some(path) = &art.banner {
        let id = upload_image(client, path).await?;
        if current.banner.as_deref() != Some(id.as_str()) {
            req.banner_image = Some(id);
        }
    }
    if req.cover_image.is_some() || req.banner_image.is_some() {
        client.update_game(game, &req).await?;
        eprintln!("Updated artwork");
    }
    Ok(())
}

fn print_games(games: &[GameSummary]) {
    for g in games {
        println!(
            "{:<24} {:<10} {:<12} {}",
            g.slug,
            g.latest_version.as_deref().unwrap_or("-"),
            g.owner,
            g.title
        );
    }
}

async fn push_cmd(ctx: &Ctx, args: PushArgs) -> Result<()> {
    let mut art = config::ProjectArt::default();
    let (game, builds, project_server) = if let Some(dir) = args.dir {
        let game = args.game.ok_or_else(|| anyhow!("--dir needs --game"))?;
        let exe = args.exe.ok_or_else(|| anyhow!("--dir needs --exe"))?;
        let platform = args
            .platform
            .or_else(|| platform::current().map(String::from))
            .ok_or_else(|| anyhow!("--dir needs --platform"))?;
        let launch = LaunchConfig { exe, args: vec![], workdir: None };
        (game, vec![PushBuild { platform, dir, launch }], None)
    } else {
        let project = ProjectFile::load(&args.config).with_context(|| {
            format!("no project file; create {} or pass --dir/--game/--exe", args.config.display())
        })?;
        let builds = project
            .builds
            .into_iter()
            .filter(|b| args.only.is_empty() || args.only.contains(&b.platform))
            .map(|b| PushBuild {
                platform: b.platform,
                dir: b.dir,
                launch: LaunchConfig { exe: b.exe, args: b.args, workdir: b.workdir },
            })
            .collect::<Vec<_>>();
        art = project.art;
        (args.game.unwrap_or(project.game), builds, project.server)
    };

    let client = ctx.client_for(project_server.as_deref()).await?;
    sync_art(&client, &game, &art).await?;
    let quiet = ctx.json;
    let report = condensate_sdk::push::push(
        &client,
        PushOptions { game, version: args.version, changelog: args.changelog, builds },
        &move |e| if !quiet { push_progress(e) },
    )
    .await?;
    ctx.print(&report, || {
        for b in &report.builds {
            println!("  {}: {} files, {}", b.platform, b.files, human_bytes(b.bytes));
        }
        println!(
            "Published {} {} (uploaded {} new of {} chunks, {})",
            report.game,
            report.version,
            report.uploaded_chunks,
            report.total_chunks,
            human_bytes(report.uploaded_bytes)
        );
    });
    Ok(())
}

fn push_progress(e: PushEvent) {
    match e {
        PushEvent::Scanning { platform } => eprintln!("Scanning {platform} build..."),
        PushEvent::Scanned { skipped, .. } => {
            for s in skipped {
                eprintln!("  skipped {s} (not a regular file)");
            }
        }
        PushEvent::Uploading { chunks, bytes } => {
            eprintln!("Uploading {chunks} new chunks ({})", human_bytes(bytes))
        }
        PushEvent::Uploaded { chunks, bytes } => progress_line(&format!("  {chunks} chunks, {}", human_bytes(bytes))),
        PushEvent::Publishing => {
            end_progress_line();
            eprintln!("Publishing...")
        }
    }
}

fn pull_progress(e: PullEvent) {
    match e {
        PullEvent::Checking => eprintln!("Checking for updates..."),
        PullEvent::Planned { files_to_write, files_to_delete, download_bytes, reused_bytes } => eprintln!(
            "{files_to_write} files to write, {files_to_delete} to remove; downloading {}, reusing {}",
            human_bytes(download_bytes),
            human_bytes(reused_bytes)
        ),
        PullEvent::Downloading { done_chunks, total_chunks, done_bytes, total_bytes } => progress_line(&format!(
            "  {done_chunks}/{total_chunks} chunks, {} / {}",
            human_bytes(done_bytes),
            human_bytes(total_bytes)
        )),
        PullEvent::Applying => {
            end_progress_line();
            eprintln!("Installing...")
        }
    }
}

fn progress_line(s: &str) {
    if std::io::stderr().is_terminal() {
        eprint!("\r{s}\x1b[K");
        let _ = std::io::stderr().flush();
    }
}

fn end_progress_line() {
    if std::io::stderr().is_terminal() {
        eprintln!();
    }
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}
