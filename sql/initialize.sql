CREATE TABLE entities (
    id INT NOT NULL PRIMARY KEY,
    parent_id INT,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    -- extra TEXT,
    
    FOREIGN KEY(parent_id) REFERENCES entities(id),
    CHECK((kind == 'file' AND parent_id IS NULL) OR
          (kind != 'file' AND parent_id IS NOT NULL)),
    UNIQUE(parent_id, name, kind)
) WITHOUT ROWID;

CREATE TABLE commits (
    id INT NOT NULL PRIMARY KEY,
    sha1 CHAR(40) NOT NULL UNIQUE,
    is_merge BOOLEAN NOT NULL,
    -- author_name TEXT,
    -- author_mail TEXT,
    author_date INT NOT NULL,
    -- commit_name TEXT,
    -- commit_mail TEXT,
    commit_date INT NOT NULL,

    has_change_info BOOLEAN NOT NULL,
    has_presence_info BOOLEAN NOT NULL,
    has_reachability_info BOOLEAN NOT NULL
) WITHOUT ROWID;

CREATE TABLE refs (
    id INT NOT NULL PRIMARY KEY,
    commit_id INT NOT NULL,
    name TEXT NOT NULL UNIQUE,

    FOREIGN KEY(commit_id) REFERENCES commits(id)
) WITHOUT ROWID;

CREATE TABLE changes (
    id INT NOT NULL PRIMARY KEY,
    commit_id INT NOT NULL,
    entity_id INT NOT NULL,
    kind CHAR NOT NULL,
    adds INT NOT NULL,
    dels INT NOT NULL,

    FOREIGN KEY(commit_id) REFERENCES commits(id),
    FOREIGN KEY(entity_id) REFERENCES entities(id),
    UNIQUE(commit_id, entity_id),
    CHECK(kind = 'A' OR kind = 'D' or kind = 'M'),
    CHECK(adds > 0 OR dels > 0)
) WITHOUT ROWID;

CREATE TABLE presence (
    commit_id INT NOT NULL,
    entity_id INT NOT NULL,
    body_range_id INT NOT NULL,
    name_range_id INT NOT NULL,

    PRIMARY KEY(commit_id, entity_id),
    FOREIGN KEY(commit_id) REFERENCES commits(id),
    FOREIGN KEY(entity_id) REFERENCES entities(id),
    FOREIGN KEY(body_range_id) REFERENCES ranges(id),
    FOREIGN KEY(name_range_id) REFERENCES ranges(id)
) WITHOUT ROWID;

CREATE TABLE reachability (
    source_id INT NOT NULL,
    target_id INT NOT NULL,

    PRIMARY KEY(source_id, target_id),
    FOREIGN KEY(source_id) REFERENCES commits(id),
    FOREIGN KEY(target_id) REFERENCES commits(id)
) WITHOUT ROWID;

CREATE TABLE ranges (
    id INT NOT NULL PRIMARY KEY,
    start_byte INT NOT NULL,
    start_col INT NOT NULL,
    start_row INT NOT NULL,
    end_byte INT NOT NULL,
    end_col INT NOT NULL,
    end_row INT NOT NULL
) WITHOUT ROWID;

CREATE TABLE meta.cli_options (
    id INT PRIMARY KEY CHECK(id = 0),
    refs TEXT,
    max_count INT,
    since TEXT,
    until TEXT,
    all BOOLEAN NOT NULL,
    branches TEXT,
    tags TEXT,
    remotes TEXT,
    glob TEXT,
    paths TEXT,
    ran_at INT NOT NULL
) WITHOUT ROWID;

CREATE TABLE meta.skipped_paths (
    path TEXT NOT NULL UNIQUE
);

CREATE TABLE updates (
    tag_id INT NOT NULL,
    max_count INT,
    since INT,
    until INT,
    paths TEXT,
    updated_at INT NOT NULL

) WITHOUT ROWID;

CREATE TABLE tool_runs (
    id INT NOT NULL PRIMARY KEY,
    
    device_hostname TEXT,
    device_platform TEXT,
    device_os TEXT,
    device_cpu TEXT,
    
    tool_start_time INT NOT NULL,
    tool_end_time INT NOT NULL,

    opt_kind VARCHAR(8) NOT NULL,
    opt_refs TEXT NOT NULL,
    opt_max_commits INT,
    opt_since INT,
    opt_until INT,
    opt_max_age INT,
    opt_min_age INT,

    CHECK(opt_kind = 'UPDATE' OR opt_kind = 'REPLACE')
) WITHOUT ROWID;


-- UPDATE
-- REPLPACE