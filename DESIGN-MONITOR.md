# Self-Replacement: Monitor Process Design

How mage upgrades itself at runtime. See also DESIGN-DISTRIBUTION.md for
install layout, wrapper script, generations.jsonl, and cleanup policy.


## Overview

The mage binary has two modes: monitor and agent. On startup, if no
MAGE_AGENT_PIPE_FD is set, the binary IS the monitor. It does not exec()
itself or re-launch. It just enters the monitor loop directly, spawning
the agent as a child process.

User runs: mage --interactive

  mage process starts
    |
    | no MAGE_AGENT_PIPE_FD env var -> I am the monitor
    |
    | read generations.jsonl -> last line -> current binary is "mage-brave-eagle"
    | create anonymous pipe: (read_fd, write_fd)
    | spawn child:
    |     ~/.mage/bin/mage-brave-eagle --interactive
    |     with MAGE_AGENT_PIPE_FD=<write_fd>
    |     inherits stdin/stdout/stderr
    |
    | wait for child to exit
    |
    | child exits with code 42?
    |   YES -> read new binary path from pipe
    |          spawn new binary
    |          run health check
    |          on pass: append to generations.jsonl, loop
    |          on fail: rollback to previous, loop
    |   NO  -> exit with same code (pass through)

The monitor is just a loop. It does not use any agent logic, any LLM types,
any tools. It reads a JSON file, spawns a binary, waits, and maybe spawns
another. This means:

  - The monitor code is trivial and stable across versions
  - The monitor binary can be an old version. Doesn't matter.
  - No exec() needed. The original process is the monitor.


## The Anonymous Pipe

Why anonymous over named:

  Anonymous pipe:
    pipe() syscall, no filesystem
    Automatic cleanup on process exit
    No path collision between sessions
    Only parent/child can access (fd inheritance)
    Works on both POSIX and Windows

  Named pipe (FIFO):
    mkfifo(), needs a filesystem path
    Must unlink manually
    Path conflicts between concurrent sessions
    Any process with path access can read/write
    Different API on Windows

Anonymous pipes are the right primitive. The monitor creates the pipe, the
child inherits the write end via fd inheritance.

Pipe protocol:

  Agent writes one line: the absolute path to the new binary, newline-terminated.
  Monitor reads until EOF (which happens when child exits and write end closes).

  No JSON, no headers, no framing. One path, one newline.

Edge cases:
  - Agent exits 42 but pipe is empty: monitor logs error, rolls back.
  - Agent writes to pipe but exits 0: monitor ignores pipe contents.
  - Agent crashes (SIGSEGV, etc.): monitor sees non-42 exit, passes through.
  - Pipe buffer full: impossible in practice. Path < 4096 bytes, pipe >= 64KB.


## Exit Code Protocol

  0            Normal exit. Monitor exits 0.
  42           Upgrade requested. Monitor reads pipe, spawns new binary.
  1-41, 43-255 Error or signal. Monitor exits with same code.

42 is chosen because:
  - Not used by any standard Unix convention
  - Memorable
  - Won't collide with signal-based exits (128+N)


## Health Check

This is the critical safety mechanism for self-replication. When the monitor
spawns a new generation, it does not trust it blindly. It verifies the new
binary is actually working before committing to it.

Timeline of an upgrade:

  1. Agent (gen N) compiles new binary, writes path to pipe, exits 42
  2. Monitor reads pipe: ~/.mage/bin/mage-brave-eagle
  3. Monitor appends to generations.jsonl: new entry with status "pending"
  4. Monitor spawns new binary with same args + MAGE_HEALTH_CHECK_FD=<fd>
  5. New binary starts, enters agent loop
  6. After startup, new binary performs self-checks:
     a. Verify internal state is consistent
     b. Verify the agent loop is running
     c. Branch from the current conversation and ask the LLM:
        "I just upgraded. Run a quick self-diagnostic.
         Are you functioning correctly? Reply YES or NO."
     d. LLM responds YES or NO
  7. New binary writes health check result to MAGE_HEALTH_CHECK_FD
  8. Monitor reads health check result

  On YES (healthy):
    Monitor appends to generations.jsonl: status "healthy"
    Monitor continues supervising this generation
    Old generation binary can be cleaned up

  On NO or timeout (unhealthy):
    Monitor kills the new binary (SIGTERM)
    Monitor appends to generations.jsonl: status "failed"
    Monitor appends previous generation with status "healthy" (rollback)
    Monitor spawns previous generation with original args
    Previous generation resumes the session
    Previous generation can report: "I tried to upgrade but the new
      version failed its health check. Rolling back."

The health check is a bidirectional channel between monitor and child.
The monitor sends "are you healthy?" (implicitly, by setting the env var).
The child responds with a result on the fd.


### Health check protocol

The health check uses a second anonymous pipe (separate from the upgrade pipe):

  Monitor creates: (health_read_fd, health_write_fd)
  Child receives: MAGE_HEALTH_CHECK_FD=<health_write_fd>

Child writes to health_write_fd:

  HEALTHY\n          self-checks passed, LLM confirmed
  UNHEALTHY\n        self-checks failed or LLM said no
  UNHEALTHY:msg\n    self-checks failed with reason

If the child never writes (crashes during startup, hangs):

  Monitor has a timeout (default: 30 seconds, configurable via
  MAGE_HEALTH_TIMEOUT or config).
  After timeout: treat as UNHEALTHY, kill child, rollback.


### What the self-check does

The new binary's health check runs before it takes over the conversation:

  1. Deserialize session state from disk
     - If this fails: UNHEALTHY (binary can't read its own session format)

  2. Initialize all extensions, register all tools
     - If any extension panics or fails init: UNHEALTHY

  3. Verify the default LLM provider is reachable
     - Make a minimal API call (e.g., count tokens on a tiny prompt)
     - If this fails: UNHEALTHY (but maybe network issue, not binary issue)
     - Could be a soft failure: warn but don't block

  4. Fork a branch from the conversation
     - Send a short message to the LLM using the current conversation context:
       "System: I have just been upgraded to generation N. Please run a brief
        self-diagnostic. Verify you can read the conversation history and that
        your tools are available. Reply YES if everything looks correct, NO
        with a brief reason if not."
     - The LLM's response is the final arbiter
     - This catches subtle issues: corrupted conversation state, missing
       tools that the conversation expects, incompatible type changes

  5. Write HEALTHY or UNHEALTHY to the health check pipe
  6. If HEALTHY: continue into the normal agent loop (resume conversation)
  7. If UNHEALTHY: exit (monitor will kill us anyway)

Step 4 is the interesting one. The LLM acts as an integration test: it can
see the conversation history, it can see the tool list, it can reason about
whether the state is consistent. This is more powerful than any static check
because the LLM understands context.

The branch is discarded — it's not added to the real conversation history.
The user never sees the health check exchange.


### Health check cost

  - One LLM API call (small: ~500 input tokens from conversation tail + system
    prompt, ~50 output tokens for YES/NO)
  - ~1-3 seconds latency
  - ~$0.001-0.005 cost per check (at current Anthropic pricing)
  - Only happens on upgrade, not on every startup

This is cheap insurance against shipping a broken self-modification.


### Rollback mechanics

When the monitor rolls back:

  1. Kill the failed new binary (SIGTERM, wait, SIGKILL if needed)
  2. Append to generations.jsonl:
     - Failed binary with status "failed"
     - Previous binary with status "healthy" (rollback)
  3. Spawn previous binary with original args
  4. Previous binary reads session, resumes conversation
  5. Previous binary can detect it was rolled back (check generations.jsonl
     or a MAGE_ROLLED_BACK=1 env var) and inform the user/LLM

If the rollback target also fails health check: the monitor gives up and
exits with an error. It does not cascade through all generations. Two
consecutive failures means something is fundamentally wrong and a human
needs to intervene.


## Monitor Implementation (pseudocode)

  fn run_monitor(args: Vec<String>) {
      let gen_file = mage_home().join("bin/generations.jsonl");
      let mut gens = read_generations(&gen_file);
      let mut current = gens.current_binary_path();

      loop {
          // Create pipes
          let (upgrade_read, upgrade_write) = pipe();
          let (health_read, health_write) = pipe();

          let child = Command::new(&current)
              .args(&args[1..])
              .env("MAGE_AGENT_PIPE_FD", upgrade_write.to_string())
              .env("MAGE_HEALTH_CHECK_FD", health_write.to_string())
              .stdin(Stdio::inherit())
              .stdout(Stdio::inherit())
              .stderr(Stdio::inherit())
              .spawn()
              .expect("Failed to spawn agent");

          // Monitor only reads. Close write ends.
          close(upgrade_write);
          close(health_write);

          let status = child.wait().expect("Failed to wait");

          match status.code() {
              Some(42) => {
                  let new_path = read_line_from(upgrade_read);
                  close(upgrade_read);

                  if new_path.is_empty() || !Path::new(&new_path).is_file() {
                      eprintln!("monitor: invalid upgrade path");
                      process::exit(1);
                  }

                  // Record as pending
                  gens.add_pending(&new_path);
                  gens.set_current(&new_path);
                  write_generations(&gen_file, &gens);

                  // Spawn new version and health check it
                  let (h_read, h_write) = pipe();
                  let new_child = Command::new(&new_path)
                      .args(&args[1..])
                      .env("MAGE_AGENT_PIPE_FD", ...)  // new pipe for next upgrade
                      .env("MAGE_HEALTH_CHECK_FD", h_write.to_string())
                      .spawn()
                      .expect("Failed to spawn new version");
                  close(h_write);

                  // Wait for health check (with timeout)
                  match read_health_check(h_read, Duration::from_secs(30)) {
                      HealthResult::Healthy => {
                          gens.mark_healthy(&new_path);
                          write_generations(&gen_file, &gens);
                          current = new_path.into();
                          // Continue supervising the new child
                          // (it's already running the agent loop)
                      }
                      HealthResult::Unhealthy(reason) => {
                          eprintln!("monitor: health check failed: {reason}");
                          new_child.kill();
                          gens.mark_failed(&new_path);
                          gens.set_current(&previous);
                          write_generations(&gen_file, &gens);
                          current = previous;
                          // Loop back to spawn the previous version
                      }
                      HealthResult::Timeout => {
                          eprintln!("monitor: health check timed out");
                          new_child.kill();
                          // same as unhealthy
                      }
                  }
              }
              Some(code) => process::exit(code),
              None => process::exit(1),  // killed by signal
          }
      }
  }


## Agent Side (requesting upgrade)

  fn request_upgrade(new_binary: &Path, session: &Session) -> ! {
      // 1. Save session state
      session.save().expect("Failed to save session");

      // 2. Write new binary path to the monitor pipe
      let pipe_fd: RawFd = env::var("MAGE_AGENT_PIPE_FD")
          .expect("Not running under monitor")
          .parse()
          .expect("Invalid pipe fd");

      let mut pipe = unsafe { File::from_raw_fd(pipe_fd) };
      writeln!(pipe, "{}", new_binary.display())
          .expect("Failed to write to monitor pipe");
      drop(pipe);

      // 3. Exit with code 42
      process::exit(42);
  }


## Agent Side (health check response)

  fn perform_health_check(session: &Session, agent: &Agent) {
      let fd_str = match env::var("MAGE_HEALTH_CHECK_FD") {
          Ok(s) => s,
          Err(_) => return,  // no health check requested (normal startup)
      };

      let health_fd: RawFd = fd_str.parse().expect("Invalid health fd");
      let mut pipe = unsafe { File::from_raw_fd(health_fd) };

      // 1. Check session deserialization
      if let Err(e) = session.verify_integrity() {
          writeln!(pipe, "UNHEALTHY:session integrity: {e}").ok();
          return;
      }

      // 2. Check extensions loaded
      if let Err(e) = agent.verify_extensions() {
          writeln!(pipe, "UNHEALTHY:extensions: {e}").ok();
          return;
      }

      // 3. Check LLM provider reachable
      if let Err(e) = agent.provider().ping().await {
          writeln!(pipe, "UNHEALTHY:provider unreachable: {e}").ok();
          return;
      }

      // 4. Fork conversation branch, ask LLM
      let diagnostic = agent.fork_branch(
          "I just upgraded to a new generation. \
           Verify the conversation history is readable and tools are available. \
           Reply YES if everything looks correct, NO with reason if not."
      ).await;

      match diagnostic.as_str() {
          s if s.starts_with("YES") => {
              writeln!(pipe, "HEALTHY").ok();
          }
          _ => {
              writeln!(pipe, "UNHEALTHY:llm diagnostic: {diagnostic}").ok();
          }
      }
  }


## When No Monitor Is Present

If MAGE_AGENT_PIPE_FD is not set, the agent is running standalone (invoked
directly for testing, under an external supervisor, or on a platform where
the monitor is disabled).

Fallback:
  1. Compile the new binary
  2. Append to generations.jsonl (status "healthy" -- no health check possible)
  3. Print: "New version compiled: mage-brave-eagle"
  4. Print: "Restart mage to use the new version."
  5. Continue running the current version

No crash, no error. Graceful degradation.


## Signal Handling

  Ctrl+C (SIGINT):
    Monitor and child are in the same process group.
    Both receive SIGINT.
    Child handles it: save session, exit cleanly.
    Monitor sees non-42 exit, exits.

  SIGTERM:
    Same as SIGINT.

  Ctrl+Z (SIGTSTP):
    Both suspend. Resume with fg.

  SIGHUP (terminal closed):
    Forwarded to child. Child saves session, exits. Monitor exits.

The monitor installs no special signal handlers. Terminal signals are
delivered to the entire process group naturally.


## Session Continuity Across Upgrades

When the agent exits 42, it has already saved the session to disk:

  ~/.mage/sessions/{session-id}/
    session.jsonl              full conversation history
    state.json                 agent state: tool registry, turn count, etc.
    extensions/                session-scoped extensions

The monitor spawns the new binary with the same args (which include the
session ID). The new binary reads the session and resumes.

The session file format is the contract between generations.


## Multiple Upgrades in One Session

  Monitor spawns Gen 0
    Gen 0 modifies a tool, compiles Gen 1
    Gen 0 exits 42
  Monitor spawns Gen 1, health check passes
    Gen 1 modifies another tool, compiles Gen 2
    Gen 1 exits 42
  Monitor spawns Gen 2, health check passes
    Gen 2 finishes the task
    Gen 2 exits 0
  Monitor exits 0

Each generation gets a fresh pair of pipes (upgrade + health check).


## Windows

Windows does not have exec(). But the monitor doesn't use exec() anyway --
it just spawns children. The same code works on Windows with:
  - CreatePipe() instead of pipe()
  - Child inherits handles via STARTUPINFO
  - No process group differences (Windows job objects for signal forwarding)


## Opting Out

  mage --no-monitor          run agent directly, no self-replacement
  MAGE_NO_MONITOR=1 mage    same, via env var

Use cases:
  - Debugging the agent process
  - Running under systemd / launchd
  - CI/testing


## Security Considerations

  - Pipes are anonymous: only monitor and its direct children can access them
  - Monitor only executes binaries the agent compiled. The agent already has
    full shell access via its tool system, so this is not an escalation.
  - New binary path must be an existing file. Monitor validates before spawn.
  - Monitor does not elevate privileges. New binary runs as same user.
  - Health check prevents a compromised/broken self-modification from persisting.
    The rollback mechanism limits the blast radius to one failed attempt.


## Summary

  mage binary       Two modes: monitor or agent, based on env vars
  Anonymous pipe     Agent -> Monitor: new binary path (upgrade pipe)
  Anonymous pipe     Child -> Monitor: HEALTHY/UNHEALTHY (health check pipe)
  Exit code 42       Agent -> Monitor: "read the pipe, restart me"
  Health check        LLM-verified self-diagnostic after each upgrade
  Rollback            Automatic on health check failure, appends to generations.jsonl
  Session JSONL       State continuity across generations
  Snapshot archive    Source continuity, embedded in each generation's binary
