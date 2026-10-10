SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- Trigger configuration errors must fail closed, including the NULL TG_ARGV[0]
-- produced by a missing argument. No new tables, grants or version guards.
CREATE OR REPLACE FUNCTION pl_set_once() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path = pg_catalog, porchlight, pg_temp
    AS $$
BEGIN
    IF TG_NARGS <> 1 OR TG_ARGV[0] IS NULL OR TG_ARGV[0] = '' THEN
        RAISE EXCEPTION 'set-once requires exactly one column argument'
            USING ERRCODE = 'PL422', TABLE = TG_TABLE_NAME;
    END IF;
    IF NOT (to_jsonb(OLD) ? TG_ARGV[0]) THEN
        RAISE EXCEPTION 'unknown set-once column %', TG_ARGV[0]
            USING ERRCODE = 'PL422', TABLE = TG_TABLE_NAME, COLUMN = TG_ARGV[0];
    END IF;
    IF to_jsonb(OLD) -> TG_ARGV[0] <> 'null'::jsonb
       AND to_jsonb(NEW) -> TG_ARGV[0] IS DISTINCT FROM to_jsonb(OLD) -> TG_ARGV[0] THEN
        RAISE EXCEPTION '% is final once set', TG_ARGV[0]
            USING ERRCODE = 'PL422', TABLE = TG_TABLE_NAME, COLUMN = TG_ARGV[0];
    END IF;
    RETURN NEW;
END $$;

-- R19: API PAT callers and raw SQL agents share the application_name attribution key.
-- Raw agents cannot forge API member/device/request/token settings; keep their M1 guard.
CREATE OR REPLACE FUNCTION pl_current_actor() RETURNS jsonb
    LANGUAGE plpgsql STABLE
    SET search_path = pg_catalog, porchlight, pg_temp
    AS $$
DECLARE
    raw_label text := nullif(current_setting('application_name', true), '');
    api_label text := nullif(current_setting('porchlight.agent_label', true), '');
    api_token text := nullif(current_setting('porchlight.token_id', true), '');
BEGIN
    IF session_user = 'porchlight_agent' THEN
        IF coalesce(raw_label, '') !~ '^agent:[a-z0-9-]{1,32}$' THEN
            RAISE EXCEPTION 'agent writes must set application_name to agent:<name>' USING ERRCODE = 'PL403';
        END IF;
        RETURN jsonb_build_object('application_name', raw_label, 'db_user', session_user::text);
    END IF;
    IF (api_label IS NULL) <> (api_token IS NULL) THEN
        RAISE EXCEPTION 'API agent attribution requires label and token id' USING ERRCODE = 'PL403';
    END IF;
    RETURN jsonb_strip_nulls(jsonb_build_object(
        'member_id', nullif(current_setting('porchlight.actor_member_id', true), ''),
        'device_id', nullif(current_setting('porchlight.device_id', true), ''),
        'request_id', nullif(current_setting('porchlight.request_id', true), ''),
        'application_name', CASE WHEN api_label IS NOT NULL THEN 'agent:' || api_label END,
        'token_id', api_token,
        'db_user', session_user::text));
END $$;

RESET search_path;
RESET ROLE;
