//! Todos with their evidence and checks.

use yi_types::goal::Goal;
use yi_types::todo::{Claim, TodoList};

/// What the agent says it is working through.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Work {
    /// The task list; each item carries its state, its evidence and its check.
    pub todos: Option<Box<TodoList>>,
    pub plan: Option<Plan>,
    /// The thread's goal, with the discoveries recorded against it.
    pub goal: Option<Box<Goal>>,
    /// Done todos held against what the session recorded.
    pub claims: Vec<Claim>,
}

/// How far the thread's plan has come.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub done: u64,
    pub total: u64,
    /// The label of the todo that is running.
    pub running: Option<String>,
}

impl Work {
    /// The todos done and the todos listed, over every phase; none with no list.
    pub fn todo_count(&self) -> Option<(usize, usize)> {
        let items = self.todos.as_ref()?.phases.iter().flat_map(|p| &p.items);
        let done = |todo: &&yi_types::plan::doc::Todo| {
            matches!(todo.state, yi_types::plan::doc::TodoState::Done { .. })
        };
        Some((items.clone().filter(done).count(), items.count()))
    }
}
