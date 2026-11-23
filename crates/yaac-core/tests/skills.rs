//! Skills in the composer: when the picker opens, what a pick inserts, which skills a sent
//! message names, and the `skills/list` round trip against the demo.
mod common;

use common::{boot, settle};
use yaac_core::demo::threads;
use yaac_core::skills::{self, Active, Detection, Entry, SkillCaps, SkillsState, Trigger};
use yaac_protocol::{InputPart, SkillInfo};

const CLAUDE: SkillCaps = SkillCaps { skills: true, inline: false };
const CODEX: SkillCaps = SkillCaps { skills: true, inline: true };
const NONE: SkillCaps = SkillCaps { skills: false, inline: false };

fn open(text: &str, caps: SkillCaps) -> Option<Active> {
    match skills::detect(text, text.len(), caps, "X") {
        Detection::Open(a) => Some(a),
        _ => None,
    }
}

fn skill(name: &str, description: &str) -> SkillInfo {
    SkillInfo { name: name.into(), description: description.into(), path: format!("/s/{name}/SKILL.md"), scope: None, argument_hint: None }
}

#[test]
fn slash_opens_only_at_the_very_start_of_the_message() {
    for caps in [CLAUDE, CODEX] {
        let a = open("/", caps).unwrap();
        assert_eq!((a.trigger, a.start, a.query.as_str()), (Trigger::Slash, 0, ""));
        assert_eq!(open("/rev", caps).unwrap().query, "rev");
        assert_eq!(open("/release-notes", caps).unwrap().query, "release-notes");
        // Not the first word, or after a newline: an ordinary message.
        assert_eq!(open("look at /rev", caps), None);
        assert_eq!(open("hi\n/rev", caps), None);
        assert_eq!(open(" /rev", caps), None);
        // The cursor is past the name: finished.
        assert_eq!(open("/rev ", caps), None);
        assert_eq!(open("/rev and more", caps), None);
    }
}

#[test]
fn paths_never_open_the_picker() {
    for text in ["/home/me/project", "/etc/hosts", "/usr/", "//", "/tmp/x.log"] {
        assert_eq!(open(text, CODEX), None, "{text}");
        assert_eq!(skills::detect(text, text.len(), NONE, "Claude Code"), Detection::None, "{text}");
    }
    // A path in the middle of a message is not the first word either.
    assert_eq!(open("see /home/me", CODEX), None);
}

#[test]
fn dollar_opens_at_a_word_start_only_with_inline_skills() {
    assert_eq!(open("$", CODEX).unwrap().trigger, Trigger::Dollar);
    let a = open("use $pl", CODEX).unwrap();
    assert_eq!((a.start, a.query.as_str()), (4, "pl"));
    assert_eq!(open("run ($pl", CODEX).unwrap().start, 5);
    // Currency, shell variables glued to words, and doubled signs are not triggers.
    for text in ["it costs $5", "$100", "$1,000 total", "price$x", "a$b", "$$", "echo $/x", "$5.00"] {
        assert_eq!(open(text, CODEX), None, "{text}");
    }
    // Claude Code has no inline skills: `$` never opens a picker.
    assert_eq!(open("use $pl", CLAUDE), None);
    assert_eq!(open("$", CLAUDE), None);
}

#[test]
fn cursor_in_the_middle_of_a_name_replaces_the_whole_word() {
    let text = "use $plan now";
    let a = match skills::detect(text, 7, CODEX, "Codex") {
        Detection::Open(a) => a,
        other => panic!("{other:?}"),
    };
    assert_eq!((a.start, a.end, a.query.as_str()), (4, 9, "pl"));
    let e = skills::apply(text, &a, &skill_entry("plan"), CODEX);
    assert_eq!(e.text, "use $plan now");
    assert_eq!(&e.text[..e.cursor], "use $plan");
}

fn skill_entry(name: &str) -> Entry {
    skills::entries(&[skill(name, "d")], &[], "").remove(0)
}

#[test]
fn unsupported_triggers_say_so_for_the_provider_and_keep_the_text() {
    // Claude Code: `$name` as the first word.
    match skills::detect("$plan", 5, CLAUDE, "Claude Code") {
        Detection::Unsupported(t) => {
            assert!(t.contains("Claude Code"));
            assert!(t.contains("/name"));
        }
        other => panic!("{other:?}"),
    }
    // A provider with no skills at all.
    match skills::detect("/plan", 5, NONE, "Gemini") {
        Detection::Unsupported(t) => assert!(t.contains("Gemini")),
        other => panic!("{other:?}"),
    }
    // Where a user would not be asking for a skill: nothing, not even a tooltip.
    for (text, caps) in [("cost is $5", CLAUDE), ("echo $HOME", CLAUDE), ("$5", CLAUDE), ("/", NONE), ("/ 5", NONE), ("/home/x", NONE), ("echo $PATH", NONE)] {
        assert_eq!(skills::detect(text, text.len(), caps, "X"), Detection::None, "{text}");
    }
}

#[test]
fn picker_rows_rank_names_before_descriptions_and_builtins_come_first() {
    let list = [skill("write-plan", "Plan first"), skill("plan", "Think"), skill("review", "Check the plan"), skill("lint", "Style")];
    let names = |q: &str, b: &[Entry]| skills::entries(&list, b, q).into_iter().map(|e| e.name).collect::<Vec<_>>();
    assert_eq!(names("", &[]), ["write-plan", "plan", "review", "lint"]);
    assert_eq!(names("pl", &[]), ["plan", "write-plan", "review"]);
    assert_eq!(names("PLAN", &[]), ["plan", "write-plan", "review"]);
    assert_eq!(names("zzz", &[]), Vec::<String>::new());
    // The Compact built-in is the caller's to offer (under the `compact` capability).
    assert_eq!(names("", &[Entry::compact()])[0], "compact");
    assert_eq!(names("comp", &[Entry::compact()]), ["compact"]);
}

#[test]
fn a_pick_inserts_slash_for_claude_and_dollar_for_codex() {
    let entry = skill_entry("plan");
    let a = open("/pl", CLAUDE).unwrap();
    let e = skills::apply("/pl", &a, &entry, CLAUDE);
    assert_eq!((e.text.as_str(), e.cursor), ("/plan ", 6));
    // Codex lists from `/` too, but names the skill with `$`.
    let a = open("/pl", CODEX).unwrap();
    assert_eq!(skills::apply("/pl", &a, &entry, CODEX).text, "$plan ");
    let a = open("fix it with $", CODEX).unwrap();
    assert_eq!(skills::apply("fix it with $", &a, &entry, CODEX).text, "fix it with $plan ");
    // A built-in command is always `/name`.
    let a = open("/co", CODEX).unwrap();
    assert_eq!(skills::apply("/co", &a, &Entry::compact(), CODEX).text, "/compact ");
    // No second space when one already follows.
    let a = match skills::detect("/pl rest", 3, CLAUDE, "C") {
        Detection::Open(a) => a,
        other => panic!("{other:?}"),
    };
    assert_eq!(skills::apply("/pl rest", &a, &entry, CLAUDE).text, "/plan rest");
}

#[test]
fn only_listed_skills_are_sent_as_structured_references() {
    let list = [skill("plan", "p"), skill("lint", "l")];
    let refs = skills::references("use $plan then $lint and $plan, not $nothing or cost $5 or a$lint", &list);
    assert_eq!(refs.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["plan", "lint"]);
    assert_eq!(refs[0].path, "/s/plan/SKILL.md");
    assert_eq!(skills::references("a sentence ending in $plan.", &list).len(), 1);
    let parts = skills::with_references(vec![InputPart::text("use $plan")], "use $plan", &list);
    assert_eq!(parts.len(), 2);
    assert!(matches!(&parts[1], InputPart::Skill(s) if s.name == "plan"));
    // No `$name` in the text: no part.
    assert_eq!(skills::with_references(vec![InputPart::text("hi")], "hi", &list).len(), 1);
}

#[test]
fn skills_are_listed_per_instance_and_folder_and_sent_with_the_message() {
    let (mut c, hub) = boot();
    let key = threads::users();
    let comp = skills::for_thread(&c.state, &key).expect("a started thread has a skills context");
    assert!(comp.caps.skills);
    c.skills_list(&comp.scope, false);
    assert_eq!(c.state.skills.get(&comp.scope), Some(&SkillsState::Loading));
    // Asking again while loading changes nothing.
    c.skills_list(&comp.scope, false);
    settle(&mut c, &hub);
    let Some(SkillsState::Ready(list)) = c.state.skills.get(&comp.scope) else { panic!("{:?}", c.state.skills.get(&comp.scope)) };
    assert!(list.iter().any(|s| s.name == "review"));
    // A project skill's path follows the folder asked about.
    assert!(list.iter().find(|s| s.name == "review").unwrap().path.starts_with(&comp.scope.cwd));
}
