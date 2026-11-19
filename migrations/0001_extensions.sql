-- 0001: Required PostgreSQL extensions
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
DO $$
BEGIN
    CREATE EXTENSION IF NOT EXISTS "pgvector";
EXCEPTION
    WHEN undefined_file OR sqlstate '0A000' THEN
        CREATE EXTENSION IF NOT EXISTS "vector";
END
$$;
CREATE EXTENSION IF NOT EXISTS "pg_trgm";
