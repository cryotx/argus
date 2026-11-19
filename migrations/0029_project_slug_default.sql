-- 0029: give `projects.slug` a safe default (V0.39 follow-up)
--
-- 0028 added `slug NOT NULL` without a default, which broke every pre-V0.39
-- insert path that never knew about slugs (the API `create_project`, the MVP
-- flow, and the E2E test helpers). V0.39 is required to be additive, so
-- instead of editing applied migration 0028, this adds a generated default:
-- existing call sites keep working and new Project State code always passes an
-- explicit, human-readable slug.
--
-- A generated `p-<12 hex>` value is unique with overwhelming probability and
-- satisfies the unique index; it is deliberately not derived from `name`
-- (which could collide after slugification).

ALTER TABLE projects
    ALTER COLUMN slug SET DEFAULT ('p-' || substr(replace(gen_random_uuid()::text, '-', ''), 1, 12));
