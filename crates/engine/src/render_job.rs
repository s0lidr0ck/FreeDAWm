//! Track › Freeze, Freeze up to here, Commit and Bounce rendered ahead of the command, a block at
//! a time, so a host can keep drawing a progress window, and through the host's live plugin
//! instances ([`soundcraft_mix::LiveInstances`]): hosted plugins render through the very
//! instances the person hears, Pro Tools offline-bounce style, instead of through copies made
//! from their stored state.
//!
//! The host builds a [`RenderJob`] for the command it is about to run, steps it every frame,
//! then calls [`RenderJob::finish`] and runs the command: [`Engine::offer_renders`] holds the
//! audio for it, and `track.freeze` / `track.commit` take it instead of rendering, as long as
//! the session is still the one the renders were made from (otherwise they render again, through
//! instances of their own, as a headless call does).

use crate::{Engine, Result};
use serde_json::Value;
use soundcraft_mix::{LiveInstances, TrackRender};
use soundcraft_model::{ModuleId, Session, TrackId};
use soundcraft_time::{Range, Samples};
use std::collections::VecDeque;
use std::sync::Arc;

/// What a render of one track is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderKind {
    /// A freeze to `end` (the end of the track's content), down to the freeze point `upto`.
    Freeze { end: Samples, upto: Option<ModuleId> },
    /// A commit over `range`.
    Commit { range: Range },
}

/// A render made ahead of a command (see [`Engine::offer_renders`]).
pub struct PreparedRender {
    pub track: TrackId,
    pub kind: RenderKind,
    /// Planar, the track's channel count.
    pub audio: Vec<Vec<f32>>,
}

/// The renders of one `track.freeze` or `track.commit` call, a block at a time.
pub struct RenderJob {
    id: &'static str,
    params: Value,
    session: Arc<Session>,
    todo: VecDeque<(TrackId, RenderKind)>,
    current: Option<(TrackId, RenderKind, TrackRender)>,
    done: Vec<PreparedRender>,
    /// The host's live instances, between one track's render and the next.
    live: Option<LiveInstances>,
    total: usize,
}

impl RenderJob {
    /// The job for command `id` (`track.freeze`, `track.commit` or `track.bounce`, which commits)
    /// with `params`, validated as the command would. `None` when the command renders nothing
    /// (a freeze that thaws, a commit of tracks with nothing to commit): run it as it is.
    pub fn new(e: &Engine, id: &str, params: &Value) -> Result<Option<RenderJob>> {
        let (id, todo): (&'static str, Vec<(TrackId, RenderKind)>) = match id {
            "track.freeze" => {
                let Some(plan) = crate::cmd::freeze::plan(e, params)? else { return Ok(None) };
                ("track.freeze", plan.todo.into_iter().map(|(t, end)| (t, RenderKind::Freeze { end, upto: plan.upto })).collect())
            }
            "track.commit" | "track.bounce" => {
                let id = if id == "track.bounce" { "track.bounce" } else { "track.commit" };
                let tracks = crate::cmd::tracks_required(e, id, params)?;
                let s = e.session();
                let mut todo = Vec::new();
                for t in tracks {
                    if let Some(tr) = s.track(t)
                        && let Some(range) = crate::io::commit_range(s, tr)?
                    {
                        todo.push((t, RenderKind::Commit { range }));
                    }
                }
                (id, todo)
            }
            other => return Err(crate::cmd::bad(other, "not a render command")),
        };
        if todo.is_empty() {
            return Ok(None);
        }
        let total = todo.len();
        Ok(Some(RenderJob {
            id,
            params: params.clone(),
            session: e.session_arc(),
            todo: todo.into(),
            current: None,
            done: Vec::new(),
            live: None,
            total,
        }))
    }

    /// Renders through the host's live plugin instances. Given after the first track's render
    /// started, the instances go straight back.
    pub fn through(mut self, live: LiveInstances) -> RenderJob {
        if self.current.is_none() && self.done.is_empty() {
            self.live = Some(live);
        }
        self
    }

    /// Whether the job renders through the host's live instances.
    pub fn is_live(&self) -> bool {
        self.live.is_some() || self.current.as_ref().is_some_and(|(_, _, r)| r.is_live())
    }

    /// The command the job renders for, and its params: run it after [`RenderJob::finish`].
    pub fn command(&self) -> (&'static str, &Value) {
        (self.id, &self.params)
    }

    /// What the progress window says: "Freezing Keys", "Committing Vox".
    pub fn label(&self) -> String {
        let verb = match self.id {
            "track.freeze" => "Freezing",
            _ => "Committing",
        };
        let track = self.current.as_ref().map(|(t, _, _)| *t).or_else(|| self.todo.front().map(|(t, _)| *t));
        match track.and_then(|t| self.session.track(t)) {
            Some(t) => format!("{verb} {}", t.name),
            None => format!("{verb}…"),
        }
    }

    /// How far along, from 0 to 1, over every track of the job.
    pub fn progress(&self) -> f32 {
        let within = self.current.as_ref().map_or(0.0, |(_, _, r)| r.progress());
        ((self.done.len() as f32 + within) / self.total.max(1) as f32).clamp(0.0, 1.0)
    }

    /// Renders blocks until `more` returns false (at least one per call). True once every track
    /// is rendered.
    pub fn step(&mut self, mut more: impl FnMut() -> bool) -> bool {
        loop {
            if self.current.is_none() {
                let Some((t, kind)) = self.todo.pop_front() else { return true };
                let live = self.live.take();
                let render = match kind {
                    RenderKind::Freeze { end, upto } => TrackRender::frozen(&self.session, t, end, upto, live),
                    RenderKind::Commit { range } => TrackRender::pre_fader(&self.session, t, range, live),
                };
                self.current = Some((t, kind, render));
            }
            let Some((_, _, render)) = self.current.as_mut() else { return true };
            if render.step()
                && let Some((t, kind, render)) = self.current.take()
            {
                let (audio, live) = render.finish();
                self.live = live;
                self.done.push(PreparedRender { track: t, kind, audio });
            }
            if self.todo.is_empty() && self.current.is_none() {
                return true;
            }
            if !more() {
                return false;
            }
        }
    }

    /// Hands the renders to `e` for the command ([`Engine::offer_renders`]) and gives the live
    /// instances back to the host. Run the command right after.
    pub fn finish(self, e: &mut Engine) {
        drop(self.current);
        drop(self.live);
        e.offer_renders(self.done);
    }

    /// Renders everything and runs the command on `e`, as a headless host would.
    pub fn run(mut self, e: &mut Engine) -> Result<Value> {
        while !self.step(|| true) {}
        let (id, params) = (self.id, self.params.clone());
        self.finish(e);
        e.execute(id, &params)
    }
}
