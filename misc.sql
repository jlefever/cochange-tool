WITH RECURSIVE filenames (entity_id, filename, level) AS
(
  SELECT E.id AS entity_id, E.name AS filename, 0 as level
  FROM entities E
  WHERE E.kind = 'file'
  
  UNION ALL
  
  SELECT E.id AS entity_id, F.filename, F.level + 1
  FROM entities E, filenames F
  WHERE E.parent_id = F.entity_id
),
fl_src_deps AS
(
  SELECT DISTINCT FS.filename AS src_file, D.tgt_id, D.kind
  FROM deps D
  JOIN filenames FS ON FS.entity_id = D.src_id
)
SELECT DISTINCT FSD.src_file AS src_name, TE.name AS tgt_name, FSD.kind AS dep_kind
FROM fl_src_deps FSD
JOIN filenames FT ON FT.entity_id = FSD.tgt_id
JOIN entities TE ON TE.id = FSD.tgt_id
WHERE
	FT.filename = 'core/java/android/view/View.java' AND
	TE.parent_id = 99234 AND
    FSD.src_file <> FT.filename

UNION

SELECT ES.name AS src_name, ET.name AS tgt_name, D.kind AS dep_kind
FROM deps D
JOIN entities ES ON ES.id = D.src_id
JOIN entities ET ON ET.id = D.tgt_id
WHERE ES.parent_id = 99234 AND ET.parent_id = 99234


-- View.java has id 99233
-- View class has id 99234
-- SELECT * FROM entities WHERE parent_id = 99234