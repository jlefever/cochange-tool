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
    CHECK(kind = 'A' OR kind = 'D' or kind = 'M')
    -- CHECK(adds > 0 OR dels > 0)
) WITHOUT ROWID;