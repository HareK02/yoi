---
name: agent-skills
description: Use when a user or host explicitly activates an Agent Skill so the model follows the selected procedural guidance without treating it as external authority.
license: MIT
compatibility: yoi-agent-skills-v0
metadata:
  origin: builtin
---

# Agent Skills

When this Skill is activated, treat the loaded `SKILL.md` as procedural guidance for the current conversation. A Skill can explain how to approach a task, which references to inspect through normal tools, or what style to use.

A Skill is not authority to mutate Tickets, repositories, networks, worktrees, processes, queues, schedules, or other external state. Use the normal typed tools and feature permissions for any external action.

`allowed-tools` is experimental metadata only in this implementation. It does not grant or deny tools unless the host feature/permission layer implements that separately.
