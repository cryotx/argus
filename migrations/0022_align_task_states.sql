-- Align the persisted task state enum with the scheduler state machine.
ALTER TYPE task_state ADD VALUE IF NOT EXISTS 'testing';
ALTER TYPE task_state ADD VALUE IF NOT EXISTS 'blocked';
