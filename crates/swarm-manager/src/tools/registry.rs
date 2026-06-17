use super::Tool;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Registry of available tools, keyed by tool name.
///
/// Backed by a `BTreeMap` (not `HashMap`) so iteration — `list`,
/// `to_openai_tools` — is ordered by tool name and therefore identical across
/// processes. The serialized tools array is part of the prompt sent to the
/// model; a per-run-random ordering would perturb weak-model tool choice and
/// break prompt reproducibility, so the order is pinned here at the source.
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
        }
    }

    /// Register a tool. A later registration with the same name replaces the
    /// earlier one.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Get a tool by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// List all registered tool names.
    pub fn list(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }

    /// Drop every registered tool whose name fails `keep`.
    ///
    /// Used to gate the tool set against a skill's `allowed-tools`: register the
    /// built-ins, then `retain(|name| allowed.contains(name))` so the agent only
    /// sees the permitted subset.
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.tools.retain(|name, _| keep(name));
    }

    /// Convert all tools to OpenAI tool-calling format.
    pub fn to_openai_tools(&self) -> Vec<Value> {
        self.tools.values().map(|t| t.to_openai_tool()).collect()
    }

    /// Execute a tool by name.
    ///
    /// v1 dispatch is direct: the permission broker is permissive, so this
    /// calls the tool without an admission step. The method shape is kept so a
    /// later arc can route through a real broker without changing callers.
    pub async fn execute(&self, name: &str, args: Value) -> anyhow::Result<String> {
        let tool = self
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Tool not found: {}", name))?;
        tool.execute(args).await
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }

        fn description(&self) -> &str {
            "Echo the message argument back to the caller"
        }

        fn parameters(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string", "description": "Text to echo" }
                },
                "required": ["message"]
            })
        }

        async fn execute(&self, args: Value) -> anyhow::Result<String> {
            let message = args["message"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing message parameter"))?;
            Ok(message.to_string())
        }
    }

    #[test]
    fn register_get_list() {
        let mut registry = ToolRegistry::new();
        assert!(registry.list().is_empty());

        registry.register(Arc::new(EchoTool));

        let names = registry.list();
        assert_eq!(names, vec!["echo"]);
        assert!(registry.get("echo").is_some());
        assert!(registry.get("missing").is_none());
    }

    #[test]
    fn retain_drops_tools_not_matching_predicate() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        assert_eq!(registry.list(), vec!["echo"]);

        // Keep only a name the registry does not have → everything drops.
        registry.retain(|name| name == "read_file");
        assert!(registry.list().is_empty());
        assert!(registry.get("echo").is_none());

        // Re-register and keep the matching name → it stays.
        registry.register(Arc::new(EchoTool));
        registry.retain(|name| name == "echo");
        assert_eq!(registry.list(), vec!["echo"]);
    }

    #[test]
    fn register_replaces_same_name() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(EchoTool));
        assert_eq!(registry.list().len(), 1);
    }

    /// A second named tool so ordering tests have something to sort.
    struct NamedTool(&'static str);

    #[async_trait]
    impl Tool for NamedTool {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            "named test tool"
        }
        fn parameters(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
        async fn execute(&self, _args: Value) -> anyhow::Result<String> {
            Ok(self.0.to_string())
        }
    }

    /// `list` and `to_openai_tools` are ordered by tool name regardless of
    /// registration order — the BTreeMap backing makes the prompt's tools array
    /// reproducible across processes.
    #[test]
    fn iteration_is_name_ordered_not_registration_ordered() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(NamedTool("zebra")));
        registry.register(Arc::new(NamedTool("alpha")));
        registry.register(Arc::new(NamedTool("mango")));

        assert_eq!(registry.list(), vec!["alpha", "mango", "zebra"]);

        let openai_tools = registry.to_openai_tools();
        let names: Vec<&str> = openai_tools
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec!["alpha", "mango", "zebra"],
            "tool order in the prompt must be name-sorted and deterministic"
        );
    }

    #[test]
    fn to_openai_tools_shape() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        let tools = registry.to_openai_tools();
        assert_eq!(tools.len(), 1);

        let tool = &tools[0];
        assert_eq!(tool["type"], "function");
        assert_eq!(tool["function"]["name"], "echo");
        assert_eq!(
            tool["function"]["description"],
            "Echo the message argument back to the caller"
        );
        assert_eq!(tool["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn tool_to_openai_tool_matches_parts() {
        let tool = EchoTool;
        let openai = tool.to_openai_tool();
        assert_eq!(openai["function"]["name"], tool.name());
        assert_eq!(openai["function"]["description"], tool.description());
        assert_eq!(openai["function"]["parameters"], tool.parameters());
    }

    #[cfg(feature = "runtime")]
    #[tokio::test]
    async fn execute_dispatches_by_name() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        let out = registry
            .execute("echo", json!({"message": "hello"}))
            .await
            .unwrap();
        assert_eq!(out, "hello");

        let err = registry.execute("missing", json!({})).await.unwrap_err();
        assert!(err.to_string().contains("Tool not found"));
    }
}
