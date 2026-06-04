//! Task tools built-in feature module.
//!
//! This is the reference path for extracting an internal built-in module into
//! the feature contribution boundary. The Pod host still owns the Pod-lifetime
//! [`tools::TaskStore`] and passes the shared handle in at construction time;
//! the module requests no sandbox/external-plugin host authorities.

use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule, ToolContribution,
    ToolDeclaration,
};

/// Construct the built-in Task tool feature module.
///
/// The returned module contributes only `TaskCreate`, `TaskUpdate`, `TaskGet`,
/// and `TaskList` through descriptor-approved tool registration. It does not
/// request host authorities; normal ToolRegistry and PreToolCall permission
/// policy still applies at call time.
pub fn task_tools_feature(task_store: tools::TaskStore) -> impl FeatureModule {
    TaskToolsFeature { task_store }
}

struct TaskToolsFeature {
    task_store: tools::TaskStore,
}

impl FeatureModule for TaskToolsFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("task-tools", "Task tools")
            .with_description("Session-lifetime task tracking builtin tools")
            .with_tool(ToolDeclaration::new(
                "TaskCreate",
                "Create a session-lifetime user-visible task",
            ))
            .with_tool(ToolDeclaration::new(
                "TaskUpdate",
                "Update a session-lifetime user-visible task",
            ))
            .with_tool(ToolDeclaration::new(
                "TaskGet",
                "Get one session-lifetime user-visible task",
            ))
            .with_tool(ToolDeclaration::new(
                "TaskList",
                "List session-lifetime user-visible tasks",
            ))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        let names = ["TaskCreate", "TaskList", "TaskGet", "TaskUpdate"];
        for (name, definition) in names
            .into_iter()
            .zip(tools::task_tools(self.task_store.clone()))
        {
            context
                .tools()
                .register(ToolContribution::new(name, definition))?;
        }
        Ok(())
    }
}
