//! Environment management

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Deployment environment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    pub name: String,
    pub description: String,
    pub variables: HashMap<String, String>,
    pub secrets: Vec<String>, // Secret names, not values
    pub auto_deploy: bool,
    pub health_check_path: Option<String>,
}

impl Environment {
    pub fn new(name: String) -> Self {
        Self {
            name: name.clone(),
            description: format!("{} environment", name),
            variables: HashMap::new(),
            secrets: Vec::new(),
            auto_deploy: false,
            health_check_path: None,
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    pub fn with_variable(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.variables.insert(key.into(), value.into());
        self
    }

    pub fn with_secret(mut self, secret: impl Into<String>) -> Self {
        self.secrets.push(secret.into());
        self
    }

    pub fn with_auto_deploy(mut self, auto_deploy: bool) -> Self {
        self.auto_deploy = auto_deploy;
        self
    }

    pub fn with_health_check(mut self, path: impl Into<String>) -> Self {
        self.health_check_path = Some(path.into());
        self
    }
}

/// Environment manager
pub struct EnvironmentManager {
    environments: HashMap<String, Environment>,
}

impl EnvironmentManager {
    pub fn new() -> Self {
        let mut manager = Self {
            environments: HashMap::new(),
        };

        // Add default environments
        manager.add(Environment::new("development".to_string()));
        manager.add(Environment::new("staging".to_string()));
        manager.add(Environment::new("production".to_string()));

        manager
    }

    pub fn add(&mut self, env: Environment) {
        self.environments.insert(env.name.clone(), env);
    }

    pub fn get(&self, name: &str) -> Option<&Environment> {
        self.environments.get(name)
    }

    pub fn list(&self) -> Vec<&Environment> {
        self.environments.values().collect()
    }

    pub fn remove(&mut self, name: &str) -> Option<Environment> {
        self.environments.remove(name)
    }
}

impl Default for EnvironmentManager {
    fn default() -> Self {
        let mut manager = Self {
            environments: HashMap::new(),
        };

        // Add default environments
        manager.add(Environment::new("development".to_string()));
        manager.add(Environment::new("staging".to_string()));
        manager.add(Environment::new("production".to_string()));

        manager
    }
}
