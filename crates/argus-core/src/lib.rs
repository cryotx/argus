//! argus-core: Core business logic for Argus
//!
//! This crate contains the Brain Engine, Task Scheduler, Event Bus,
//! Configuration management, and shared types used across all Argus crates.

pub mod accounting_state;
pub mod artifact_repository;
pub mod brain;
pub mod budget_service;
pub mod checkpoint;
pub mod complexity;
pub mod config;
pub mod context;
pub mod decision;
pub mod decomposer;
pub mod dependency_graph;
pub mod evaluation;
pub mod event_bus;
pub mod events;
pub mod execution_loop;
pub mod execution_rules;
pub mod execution_runtime;
pub mod intelligence;
pub mod intent;
pub mod llm_client;
pub mod multi_project;
pub mod mvp;
pub mod planner;
pub mod priority_queue;
pub mod project_state;
pub mod provider;
pub mod quality;
pub mod self_evolution;
pub mod spend_repository;
pub mod task_repository;
pub mod task_scheduler;
pub mod task_types;
pub mod types;
pub mod worker_types;
pub mod workspace_manager;
pub mod workspace_repository;
