//! Projects (docs/05-threads.md#projects): folders the user created, with names the user gave
//! them, and which project a thread belongs to.
//!
//! **SEAM (wire field missing):** projects are stored on the target, but the protocol carries
//! no project registry yet (`ThreadSummary.project` is the git identity, not a user project).
//! Until it does, the window keeps them in [`crate::state::AppState::projects`] and the demo
//! fills them.

use yaac_protocol::TargetId;

/// Where a folder sits inside a git repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoLocation {
    /// The repository (its shared git directory), on that machine.
    pub repo: String,
    /// The folder relative to its checkout's root: "." for the root, "services/billing".
    pub relative: String,
    /// The checkout's name, when it is not the main one ("feature-x").
    pub checkout: Option<String>,
}

/// A project the user created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    /// Stable id.
    pub id: String,
    /// The name the user gave it.
    pub name: String,
    /// Its machine.
    pub target: TargetId,
    /// Its folder.
    pub folder: String,
    /// Where that folder sits in a repository, when it is in one.
    pub repo: Option<RepoLocation>,
}

/// Where a thread runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    /// The machine.
    pub target: TargetId,
    /// The working directory.
    pub folder: String,
    /// Its place in a repository, when it is in one.
    pub repo: Option<RepoLocation>,
}

fn components(path: &str) -> Vec<&str> {
    path.split(['/', '\\']).filter(|c| !c.is_empty() && *c != ".").collect()
}

/// Whether `inner` is `outer` or inside it, by whole path components.
pub fn contains(outer: &str, inner: &str) -> bool {
    let (o, i) = (components(outer), components(inner));
    o.len() <= i.len() && o.iter().zip(&i).all(|(a, b)| a == b)
}

/// The project a thread belongs to: first the repository rule (same repository and the
/// thread's relative path inside the project's — whichever checkout it is in), then the folder
/// rule (the deepest project folder containing the thread's folder). `None`: a temporary
/// project.
pub fn project_for<'a>(projects: &'a [Project], loc: &Location) -> Option<&'a Project> {
    let same_machine = projects.iter().filter(|p| p.target == loc.target);
    if let Some(trepo) = &loc.repo {
        let best = same_machine
            .clone()
            .filter_map(|p| {
                let prepo = p.repo.as_ref()?;
                (prepo.repo == trepo.repo && contains(&prepo.relative, &trepo.relative))
                    .then(|| (components(&prepo.relative).len(), p))
            })
            .max_by_key(|(depth, _)| *depth);
        if let Some((_, p)) = best {
            return Some(p);
        }
    }
    same_machine
        .filter(|p| contains(&p.folder, &loc.folder))
        .max_by_key(|p| components(&p.folder).len())
}

/// The last folder name, for prefilling "Name" in Create project.
pub fn suggested_name(folder: &str) -> String {
    components(folder).last().map_or_else(|| folder.to_owned(), |s| (*s).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proj(name: &str, folder: &str, repo: Option<(&str, &str)>) -> Project {
        Project {
            id: name.to_owned(),
            name: name.to_owned(),
            target: TargetId::new("m"),
            folder: folder.to_owned(),
            repo: repo.map(|(r, rel)| RepoLocation { repo: r.to_owned(), relative: rel.to_owned(), checkout: None }),
        }
    }

    fn at(folder: &str, repo: Option<(&str, &str)>) -> Location {
        Location {
            target: TargetId::new("m"),
            folder: folder.to_owned(),
            repo: repo.map(|(r, rel)| RepoLocation { repo: r.to_owned(), relative: rel.to_owned(), checkout: Some("wt".into()) }),
        }
    }

    #[test]
    fn the_deepest_containing_folder_wins() {
        let home_and_abc = [proj("~", "~", None), proj("abc", "~/projects/abc", None)];
        let abc = [proj("abc", "~/projects/abc", None)];
        let cases: [(&[Project], &str, Option<&str>); 4] = [
            (&home_and_abc, "~/projects/abc/123", Some("abc")),
            (&home_and_abc, "~/projects", Some("~")),
            (&abc, "~/projects/abc/123/src", Some("abc")),
            (&abc, "~/other", None),
        ];
        for (projects, folder, want) in cases {
            assert_eq!(project_for(projects, &at(folder, None)).map(|p| p.name.as_str()), want, "{folder}");
        }
        // Whole components only: abc does not contain abcdef.
        assert_eq!(project_for(&abc, &at("~/projects/abcdef", None)), None);
    }

    #[test]
    fn a_repository_match_beats_an_outer_folder() {
        let yaac = proj("yaac", "~/code/yaac", Some(("yaac.git", ".")));
        let billing = proj("Billing", "~/code/mono/services/billing", Some(("mono.git", "services/billing")));
        let home = proj("Home", "~", None);
        let cases: [(&[Project], Location, Option<&str>); 5] = [
            (std::slice::from_ref(&yaac), at("~/code/yaac-wt/feature-x", Some(("yaac.git", "."))), Some("yaac")),
            (std::slice::from_ref(&yaac), at("~/.codex/worktrees/a1b2/yaac/crates", Some(("yaac.git", "crates"))), Some("yaac")),
            (std::slice::from_ref(&billing), at("~/wt/hotfix/services/billing/src", Some(("mono.git", "services/billing/src"))), Some("Billing")),
            (std::slice::from_ref(&billing), at("~/wt/hotfix/services/search", Some(("mono.git", "services/search"))), None),
            (&[home, yaac.clone()], at("~/code/yaac-wt/feature-x", Some(("yaac.git", "."))), Some("yaac")),
        ];
        for (projects, loc, want) in cases {
            assert_eq!(project_for(projects, &loc).map(|p| p.name.as_str()), want, "{loc:?}");
        }
        // Another machine's projects never match.
        let mut other = at("~/code/yaac", None);
        other.target = TargetId::new("elsewhere");
        assert_eq!(project_for(std::slice::from_ref(&yaac), &other), None);
        assert_eq!(suggested_name("~/projects/abc/123"), "123");
    }
}
