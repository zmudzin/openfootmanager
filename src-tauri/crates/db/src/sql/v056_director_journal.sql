-- Decisions the morning director already made, so the next day remembers them
-- and the transfer market does not bid again for a club that already chose.
ALTER TABLE game_meta ADD COLUMN director_journal_json TEXT NOT NULL DEFAULT '[]';
