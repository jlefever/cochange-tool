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