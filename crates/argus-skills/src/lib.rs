//! argus-skills: Skills runtime, built-in skills, skill generator

pub mod builtin;
pub mod generator;
pub mod manifest;
pub mod registry;
pub mod runtime;

#[cfg(test)]
mod tests {
    use crate::builtin::builtin_skills;
    use crate::generator::SkillGenerator;
    use crate::registry::SkillRegistry;

    #[test]
    fn test_builtin_skills_non_empty() {
        let skills = builtin_skills();
        assert!(!skills.is_empty());
    }

    #[test]
    fn test_registry_roundtrip() {
        let reg = SkillRegistry::new();
        let skill = SkillGenerator::new().generate_from_description("hello", "desc", "echo hi");
        reg.register(skill.clone()).unwrap();
        let fetched = reg.get("hello").unwrap().unwrap();
        assert_eq!(fetched.id, skill.id);
    }
}
