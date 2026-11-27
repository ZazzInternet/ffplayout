-- Web pages that can be scheduled in a playlist as `web://<id>`.
CREATE TABLE config_web_pages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_id INTEGER NOT NULL REFERENCES channels (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    url TEXT NOT NULL,
    steps TEXT NOT NULL DEFAULT '[]' CHECK (json_valid (steps)),
    audio INTEGER NOT NULL DEFAULT 1 CHECK (audio IN (0, 1)),
    zoom REAL NOT NULL DEFAULT 1.0 CHECK (zoom BETWEEN 0.25 AND 4.0),
    preroll INTEGER NOT NULL DEFAULT 20 CHECK (preroll BETWEEN 5 AND 300)
);

CREATE INDEX config_web_pages_channel ON config_web_pages (channel_id);
