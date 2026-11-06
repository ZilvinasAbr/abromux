//! GitLab REST API v4 responses: only the fields the plugin uses. Unknown
//! fields are ignored, and most known ones default, so newer or older
//! GitLabs still parse.

use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct User {
    pub id: u64,
    pub username: String,
    pub name: String,
    pub avatar_url: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct References {
    /// `group/project!1`
    pub full: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct TaskCompletion {
    pub count: u32,
    pub completed_count: u32,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Milestone {
    pub title: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct DiffRefs {
    pub base_sha: String,
    pub head_sha: String,
    pub start_sha: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct MergeRequest {
    pub id: u64,
    pub iid: u64,
    pub project_id: u64,
    pub title: String,
    pub description: Option<String>,
    /// opened, closed, merged, locked
    pub state: String,
    pub draft: bool,
    pub author: User,
    pub assignees: Vec<User>,
    pub reviewers: Vec<User>,
    pub milestone: Option<Milestone>,
    pub source_branch: String,
    pub target_branch: String,
    #[serde(deserialize_with = "labels")]
    pub labels: Vec<Label>,
    pub references: References,
    pub web_url: String,
    pub created_at: String,
    pub updated_at: String,
    pub user_notes_count: u32,
    /// 👍 and 👎 on the merge request.
    pub upvotes: u32,
    pub downvotes: u32,
    /// The description's checklist: "1 of 2 checklist items completed".
    pub task_completion_status: Option<TaskCompletion>,
    /// e.g. mergeable, not_approved, draft_status, conflict
    pub detailed_merge_status: Option<String>,
    pub has_conflicts: bool,
    /// Who merged or closed it, when, and the merge's commit.
    pub merged_by: Option<User>,
    pub merged_at: Option<String>,
    pub merge_commit_sha: Option<String>,
    pub squash_commit_sha: Option<String>,
    pub closed_by: Option<User>,
    pub closed_at: Option<String>,
    /// The author ticked "Delete source branch when merge request is accepted".
    pub force_remove_source_branch: bool,
    pub should_remove_source_branch: Option<bool>,
    /// Only on the single-MR endpoint.
    pub diff_refs: Option<DiffRefs>,
    /// The latest pipeline for the source branch's head; single-MR endpoint only.
    pub head_pipeline: Option<Pipeline>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Pipeline {
    pub id: u64,
    pub iid: u64,
    /// created, waiting_for_resource, preparing, pending, running, success,
    /// failed, canceled, skipped, manual, scheduled
    pub status: String,
    pub sha: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub web_url: String,
}

impl Pipeline {
    /// GitLab's word for the status, and how it colors it.
    pub fn label(&self) -> (&'static str, PipelineTone) {
        match self.status.as_str() {
            "success" => ("passed", PipelineTone::Good),
            "failed" => ("failed", PipelineTone::Bad),
            "running" => ("running", PipelineTone::Busy),
            "pending" | "waiting_for_resource" | "preparing" | "created" | "scheduled" => ("pending", PipelineTone::Waiting),
            "canceled" | "canceling" => ("canceled", PipelineTone::Neutral),
            "skipped" => ("skipped", PipelineTone::Neutral),
            "manual" => ("manual action needed", PipelineTone::Neutral),
            _ => ("unknown", PipelineTone::Neutral),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PipelineTone {
    Good,
    Bad,
    Busy,
    Waiting,
    Neutral,
}

/// One file of `merge_requests/:iid/diffs`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct FileDiff {
    pub old_path: String,
    pub new_path: String,
    pub a_mode: String,
    pub b_mode: String,
    // The flags are null instead of false from some endpoints (compare).
    #[serde(deserialize_with = "null_as_default")]
    pub new_file: bool,
    #[serde(deserialize_with = "null_as_default")]
    pub deleted_file: bool,
    #[serde(deserialize_with = "null_as_default")]
    pub renamed_file: bool,
    #[serde(deserialize_with = "null_as_default")]
    pub generated_file: bool,
    /// Too big for GitLab to send its diff.
    #[serde(deserialize_with = "null_as_default")]
    pub too_large: bool,
    /// Big enough that GitLab folds it by default.
    #[serde(deserialize_with = "null_as_default")]
    pub collapsed: bool,
    /// Unified diff hunks, without the `---`/`+++` header.
    pub diff: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Position {
    pub base_sha: String,
    pub start_sha: String,
    pub head_sha: String,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    /// text, image or file
    pub position_type: String,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Suggestion {
    pub from_line: u32,
    pub to_line: u32,
    pub from_content: String,
    pub to_content: String,
    pub applied: bool,
    pub appliable: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Note {
    pub id: u64,
    /// DiffNote, DiscussionNote, or null for a plain comment.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub body: String,
    pub author: User,
    pub created_at: String,
    /// GitLab's own notes ("requested review from …", "approved this …").
    pub system: bool,
    pub resolvable: bool,
    pub resolved: bool,
    pub resolved_by: Option<User>,
    pub resolved_at: Option<String>,
    pub position: Option<Position>,
    #[serde(deserialize_with = "null_as_empty")]
    pub suggestions: Vec<Suggestion>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Discussion {
    pub id: String,
    /// A lone comment that can't be replied to in a thread.
    pub individual_note: bool,
    pub notes: Vec<Note>,
}

impl Discussion {
    /// Only GitLab's own notes: hidden in the review.
    pub fn is_system(&self) -> bool {
        self.notes.iter().all(|n| n.system)
    }

    /// Where its first note sits in the diff, if it's on a line.
    pub fn position(&self) -> Option<&Position> {
        self.notes.first()?.position.as_ref()
    }

    pub fn resolvable(&self) -> bool {
        self.notes.iter().any(|n| n.resolvable)
    }

    pub fn resolved(&self) -> bool {
        self.resolvable() && self.notes.iter().filter(|n| n.resolvable).all(|n| n.resolved)
    }

    /// Who resolved it and when.
    pub fn resolution(&self) -> Option<(&User, &str)> {
        let n = self.notes.iter().find(|n| n.resolved && n.resolved_by.is_some())?;
        Some((n.resolved_by.as_ref()?, n.resolved_at.as_deref().unwrap_or_default()))
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Approver {
    pub user: User,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Approvals {
    pub approved: bool,
    pub approved_by: Vec<Approver>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Commit {
    pub id: String,
    pub short_id: String,
    pub title: String,
    /// The whole message: the title, a blank line, the body.
    pub message: String,
    pub author_name: String,
    pub author_email: String,
    pub authored_date: String,
    pub created_at: String,
}

impl Commit {
    /// The message after the title, if any.
    pub fn body(&self) -> Option<&str> {
        let rest = self.message.trim_end().split_once('\n')?.1.trim();
        (!rest.is_empty()).then_some(rest)
    }
}

/// A merge request was closed, reopened or merged (GitLab keeps these
/// apart from notes, but shows them in the activity like one).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct StateEvent {
    pub id: u64,
    pub user: Option<User>,
    pub created_at: String,
    /// closed, reopened, merged
    pub state: String,
}

/// One push to a merge request: what the version picker lists.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct MrVersion {
    pub id: u64,
    pub head_commit_sha: String,
    pub base_commit_sha: String,
    pub start_commit_sha: String,
    pub created_at: String,
}

/// A version's diffs, or a comparison's.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Diffs {
    pub diffs: Vec<FileDiff>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Member {
    pub id: u64,
    pub access_level: u32,
}

impl Member {
    /// The role badge GitLab shows on this member's notes.
    pub fn role(&self) -> Option<&'static str> {
        Some(match self.access_level {
            50.. => "Owner",
            40..=49 => "Maintainer",
            30..=39 => "Developer",
            20..=29 => "Reporter",
            15..=19 => "Planner",
            10..=14 => "Guest",
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Version {
    pub version: String,
}

/// A label: its name and, with `with_labels_details`, its colors.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Label {
    pub name: String,
    /// `#1068bf`
    pub color: String,
    pub text_color: String,
}

/// Labels as names (`["bug"]`) or as objects with colors.
fn labels<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Label>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum One {
        Name(String),
        Full(Label),
    }
    let all = Option::<Vec<One>>::deserialize(d)?.unwrap_or_default();
    Ok(all
        .into_iter()
        .map(|l| match l {
            One::Name(name) => Label { name, ..Default::default() },
            One::Full(l) => l,
        })
        .collect())
}

fn null_as_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

fn null_as_empty<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
}
