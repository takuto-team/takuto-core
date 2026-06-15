-- Plan-11 step 2 — hand-translated port of MIGRATION_V5 (plan-10
-- per-user repositories). Adds the `repositories` registry and reshapes
-- the v1 `user_repositories` table to FK to it via composite PK.
--
-- The DROP of v1's `user_repositories` is grandfathered per plan-11 §7.5
-- because plan-01 reserved the table but no code ever wrote to it.

-- `repo_url` and `local_path` use VARCHAR(512) rather than TEXT so the
-- UNIQUE constraint and the secondary index work on MySQL without a
-- prefix length (MySQL caps index keys at 3072 bytes for InnoDB; at
-- utf8mb4 that is 768 chars, so 512 leaves a comfortable margin).
CREATE TABLE repositories (
    id VARCHAR(64) PRIMARY KEY NOT NULL,
    name VARCHAR(255) NOT NULL,
    repo_url VARCHAR(512),
    local_path VARCHAR(512) NOT NULL UNIQUE,
    default_branch VARCHAR(255) NOT NULL DEFAULT 'main',
    created_at BIGINT NOT NULL,
    created_by VARCHAR(64),
    FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL
);
CREATE INDEX idx_repositories_name ON repositories(name);
CREATE INDEX idx_repositories_repo_url ON repositories(repo_url);

DROP TABLE IF EXISTS user_repositories;

CREATE TABLE user_repositories (
    user_id VARCHAR(64) NOT NULL,
    repository_id VARCHAR(64) NOT NULL,
    added_at BIGINT NOT NULL,
    PRIMARY KEY (user_id, repository_id),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY (repository_id) REFERENCES repositories(id) ON DELETE CASCADE
);
CREATE INDEX idx_user_repositories_repo ON user_repositories(repository_id);
