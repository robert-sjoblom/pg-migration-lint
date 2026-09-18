-- PGM025: DROP COLUMN silently removes EXCLUDE constraint (room participates in EXCLUDE from V001)
ALTER TABLE room_bookings DROP COLUMN room;
