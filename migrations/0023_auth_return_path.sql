-- The same-origin path a sign-in returns to. It is kept with the attempt, so only the
-- browser that started the sign-in can choose where the finished sign-in lands.
ALTER TABLE auth_attempts ADD COLUMN return_path TEXT NOT NULL DEFAULT '/';
