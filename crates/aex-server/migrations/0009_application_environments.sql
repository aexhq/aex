ALTER TABLE http_grants ALTER COLUMN binding DROP NOT NULL;
ALTER TABLE http_grants ADD COLUMN declaration TEXT;
ALTER TABLE http_grants ADD CONSTRAINT http_grants_source CHECK ((binding IS NULL) <> (declaration IS NULL));
