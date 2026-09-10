use chrono::{Duration, Local};
use margins::session::{self, SESSION_ARTIFACT_KIND_TRANSCRIPT};
use std::path::{Path, PathBuf};

fn main() -> anyhow::Result<()> {
    let args = Args::parse()?;
    margins::initialize_sqlite_runtime()?;
    stage_fixture(&args)
}

struct Args {
    work_dir: PathBuf,
    fixture_dir: PathBuf,
    name: String,
    event_title: String,
    people: Vec<String>,
}

impl Args {
    fn parse() -> anyhow::Result<Self> {
        let mut work_dir = None;
        let mut fixture_dir = None;
        let mut name = None;
        let mut event_title = None;
        let mut people = Vec::new();
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--work-dir" => work_dir = args.next().map(PathBuf::from),
                "--fixture-dir" => fixture_dir = args.next().map(PathBuf::from),
                "--name" => name = args.next(),
                "--event-title" => event_title = args.next(),
                "--people-json" => {
                    let raw = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--people-json requires a value"))?;
                    people = serde_json::from_str(&raw)?;
                }
                other => anyhow::bail!("unknown argument: {other}"),
            }
        }
        Ok(Self {
            work_dir: work_dir.ok_or_else(|| anyhow::anyhow!("missing --work-dir"))?,
            fixture_dir: fixture_dir.ok_or_else(|| anyhow::anyhow!("missing --fixture-dir"))?,
            name: name.ok_or_else(|| anyhow::anyhow!("missing --name"))?,
            event_title: event_title.ok_or_else(|| anyhow::anyhow!("missing --event-title"))?,
            people,
        })
    }
}

fn stage_fixture(args: &Args) -> anyhow::Result<()> {
    let margins_dir = args.work_dir.join(".margins");
    std::fs::create_dir_all(&margins_dir)?;

    let notes_path = format!(".margins/{}.md", args.name);
    if !session::session_exists(&margins_dir, &args.name)? {
        session::create_session(&margins_dir, &args.name, &Local::now(), &notes_path)?;
    }
    let meta = session::get_session_meta(&margins_dir, &args.name)?;
    if meta.segments.is_empty() {
        session::add_segment(
            &margins_dir,
            &args.name,
            0,
            &format!(".margins/recordings/{}_fixture.wav", args.name),
            0,
            Some(1.0),
        )?;
    }
    session::set_title(&margins_dir, &args.name, Some(args.event_title.clone()))?;
    session::set_people(&margins_dir, &args.name, args.people.clone())?;

    let memo = std::fs::read_to_string(args.fixture_dir.join("memo.md"))?;
    let memo_path = margins_dir.join(format!("{}.md", args.name));
    std::fs::write(&memo_path, memo)?;

    let aligned = std::fs::read_to_string(args.fixture_dir.join("aligned.md"))?;
    let timeline = fixture_timeline(&aligned);
    let transcript = format!(
        "# Transcript\n\nSession: `{}`\nSource: Local headless e2e fixture.\nTranscript source: `fixture`\n\n## Timeline\n\n{}\n",
        args.name, timeline
    );
    let transcript_path = margins_dir
        .join("artifacts")
        .join(&args.name)
        .join("transcript.md");
    write_utf8(&transcript_path, &transcript)?;
    session::upsert_session_artifact(
        &margins_dir,
        &args.name,
        SESSION_ARTIFACT_KIND_TRANSCRIPT,
        0,
        &format!(".margins/artifacts/{}/transcript.md", args.name),
        "durable",
        None,
    )?;

    let capture_context = format!(
        "# Capture Context\n\nSession: `{}`\nSource: Margins Desktop memo and live transcript state.\nTranscript source: `fixture`\nDecoded until: 00:09\nCommitted until: 00:09\n\n## Timeline\n\n{}\n",
        args.name, timeline
    );
    let capture_path = margins_dir
        .join("artifacts")
        .join(&args.name)
        .join("scratch")
        .join("capture-context.md");
    write_utf8(&capture_path, &capture_context)?;
    let expires_at = (Local::now() + Duration::days(7)).to_rfc3339();
    session::upsert_session_artifact(
        &margins_dir,
        &args.name,
        "capture_context",
        0,
        &format!(
            ".margins/artifacts/{}/scratch/capture-context.md",
            args.name
        ),
        "temporary",
        Some(&expires_at),
    )?;

    println!(
        "{}",
        serde_json::json!({
            "session": args.name,
            "work_dir": args.work_dir,
            "margins_dir": margins_dir,
            "memo_path": memo_path,
            "transcript_path": transcript_path,
            "capture_context_path": capture_path,
        })
    );
    Ok(())
}

fn write_utf8(path: &Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

fn fixture_timeline(aligned: &str) -> String {
    aligned
        .lines()
        .filter_map(convert_timeline_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn convert_timeline_line(line: &str) -> Option<String> {
    let line = line.trim();
    let rest = line.strip_prefix('[')?;
    let (time, rest) = rest.split_once("] ")?;
    let (speaker, text) = rest.split_once(": ")?;
    let label = match speaker {
        "ch0" => "you (mic)",
        "ch1" => "speaker",
        "memo" => "memo",
        other => other,
    };
    Some(format!("[{time}] {label}: {text}"))
}
