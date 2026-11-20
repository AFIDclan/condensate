mod config;

use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use condensate_core::api::*;
use condensate_core::{platform, LaunchConfig};
use condensate_sdk::pull::{PullEvent, PullOptions};
use condensate_sdk::push::{PushBuild, PushEvent, PushOptions};
use condensate_sdk::Client;

use crate::config::{ProjectFile, Saved};

/// Condensate: share games with friends.
///
/// Authentication comes from `condensate login`, or from the CONDENSATE_SERVER and
/// CONDENSATE_TOKEN environment variables (handy for build agents with a publish token).
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
    /// Log in and remember the session. Password from CONDENSATE_PASSWORD or a prompt.
    Login { username: String },
    /// Forget the saved session
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
    /// Edit a game's store page
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
    },
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
    saved: Saved,
    server_override: Option<String>,
    token_override: Option<String>,
    json: bool,
}

impl Ctx {
    fn server(&self, project: Option<&str>) -> Result<String> {
        self.server_override
            .clone()
            .or_else(|| project.map(String::from))
            .or_else(|| self.saved.server.clone())
            .ok_or_else(|| anyhow!("no server configured: pass --server or set CONDENSATE_SERVER"))
    }

    fn client(&self) -> Result<Client> {
        self.client_for(None)
    }

    fn client_for(&self, project_server: Option<&str>) -> Result<Client> {
        let server = self.server(project_server)?;
        let token = self.token_override.clone().or_else(|| {
            // Only reuse the saved session for the server it belongs to.
            (self.saved.server.as_deref() == Some(server.as_str())).then(|| self.saved.token.clone()).flatten()
        });
        if token.is_none() {
            bail!("not logged in: run `condensate login <username>` or set CONDENSATE_TOKEN");
        }
        Client::new(&server, token)
    }

    fn anon_client(&self) -> Result<Client> {
        Client::new(&self.server(None)?, None)
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
    let ctx = Ctx { saved: Saved::load()?, server_override: cli.server, token_override: cli.token, json: cli.json };

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
            if let Ok(client) = ctx.client() {
                let _ = client.logout().await;
            }
            let mut saved = ctx.saved;
            saved.token = None;
            saved.save()?;
            println!("Logged out");
        }
        Command::Whoami => {
            let client = ctx.client()?;
            let me = client.me().await?;
            ctx.print(&me, || println!("{} ({}) on {}", me.username, me.display_name, client.server()));
        }
        Command::Invite(InviteCmd::Create) => {
            let invite = ctx.client()?.create_invite().await?;
            ctx.print(&invite, || println!("{}", invite.code));
        }
        Command::Invite(InviteCmd::List) => {
            let invites = ctx.client()?.list_invites().await?;
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
            let created = ctx.client()?.create_token(&CreateTokenRequest { name, game }).await?;
            ctx.print(&created, || {
                println!("{}", created.token);
                eprintln!("Save this now; it is not shown again. Use it as CONDENSATE_TOKEN.");
            });
        }
        Command::Token(TokenCmd::List) => {
            let tokens = ctx.client()?.list_tokens().await?;
            ctx.print(&tokens, || {
                for t in &tokens {
                    let scope = t.game.as_deref().map(|g| format!(" game={g}")).unwrap_or_default();
                    println!("{:>5}  {:<8} {}{}", t.id, t.kind, t.name, scope);
                }
            });
        }
        Command::Token(TokenCmd::Revoke { id }) => {
            ctx.client()?.revoke_token(id).await?;
            println!("Revoked token {id}");
        }
        Command::Game(cmd) => game_cmd(&ctx, cmd).await?,
        Command::Push(args) => push_cmd(&ctx, args).await?,
        Command::Pull { game, dir, platform, repair } => {
            let platform = platform
                .or_else(|| platform::current().map(String::from))
                .ok_or_else(|| anyhow!("unsupported platform; pass --platform"))?;
            let client = ctx.client()?;
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
            let mut child = condensate_sdk::launch::launch(&dir)?;
            let status = child.wait()?;
            if !status.success() {
                bail!("game exited with {status}");
            }
        }
        Command::Versions { game } => {
            let versions = ctx.client()?.versions(&game).await?;
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
    let saved = Saved {
        server: Some(client.server().to_string()),
        token: Some(resp.token.clone()),
        username: Some(resp.user.username.clone()),
    };
    saved.save()
}

async fn game_cmd(ctx: &Ctx, cmd: GameCmd) -> Result<()> {
    let client = ctx.client()?;
    match cmd {
        GameCmd::Create { slug, title, summary, description, visibility } => {
            let game = client.create_game(&CreateGameRequest { slug, title, summary, description, visibility }).await?;
            ctx.print(&game, || println!("Created {} ({})", game.summary.title, game.summary.slug));
        }
        GameCmd::Edit { slug, title, summary, description, visibility } => {
            let game = client.update_game(&slug, &UpdateGameRequest { title, summary, description, visibility }).await?;
            ctx.print(&game, || println!("Updated {}", game.summary.slug));
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
        (args.game.unwrap_or(project.game), builds, project.server)
    };

    let client = ctx.client_for(project_server.as_deref())?;
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
