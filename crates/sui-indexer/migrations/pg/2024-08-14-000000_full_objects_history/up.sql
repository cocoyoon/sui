-- This table will store every history version of each object, and never get pruned.
-- Since it can grow indefinitely, we keep minimum amount of information in this table for the purpose
-- of point lookups.
CREATE TABLE full_objects_history (
    object_id                   bytea         NOT NULL,
    object_version              bigint        NOT NULL,
    object_status               smallint      NOT NULL,
    serialized_object           bytea,
    CONSTRAINT full_objects_history_pk PRIMARY KEY (object_id, object_version)
);

INSERT INTO full_objects_history (object_id, object_version, object_status, serialized_object)
SELECT object_id, object_version, object_status, serialized_object
FROM objects_history;

CREATE INDEX full_objects_history_id_version ON full_objects_history (object_id, object_version);
