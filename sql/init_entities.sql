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