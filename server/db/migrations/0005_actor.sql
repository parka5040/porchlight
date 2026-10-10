SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- The actor recorded by audit triggers. The app passes member/device/request ids in transaction-local settings;
-- agents can set those to anything, so for agents they are ignored and the label is re-checked every time the
-- actor is captured (clearing application_name mid-statement aborts the write with PL403).
CREATE FUNCTION pl_current_actor() RETURNS jsonb
    LANGUAGE plpgsql STABLE
    AS $$
DECLARE
    label text := nullif(current_setting('application_name', true), '');
BEGIN
    IF session_user = 'porchlight_agent' THEN
        IF coalesce(label, '') !~ '^agent:[a-z0-9-]{1,32}$' THEN
            RAISE EXCEPTION 'agent writes must set application_name to agent:<name>' USING ERRCODE = 'PL403';
        END IF;
        RETURN jsonb_build_object('application_name', label, 'db_user', session_user::text);
    END IF;
    RETURN jsonb_strip_nulls(jsonb_build_object(
        'member_id', nullif(current_setting('porchlight.actor_member_id', true), ''),
        'device_id', nullif(current_setting('porchlight.device_id', true), ''),
        'request_id', nullif(current_setting('porchlight.request_id', true), ''),
        'application_name', label,
        'db_user', session_user::text));
END $$;

CREATE FUNCTION pl_require_agent_label() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF session_user = 'porchlight_agent'
       AND coalesce(current_setting('application_name', true), '') !~ '^agent:[a-z0-9-]{1,32}$' THEN
        RAISE EXCEPTION 'agent writes must set application_name to agent:<name>'
            USING ERRCODE = 'PL403', TABLE = TG_TABLE_NAME;
    END IF;
    RETURN NULL;
END $$;

CREATE TRIGGER aa_require_agent_label BEFORE INSERT OR UPDATE OR DELETE ON items
    FOR EACH STATEMENT EXECUTE FUNCTION pl_require_agent_label();
CREATE TRIGGER aa_require_agent_label BEFORE INSERT OR UPDATE OR DELETE ON item_assignees
    FOR EACH STATEMENT EXECUTE FUNCTION pl_require_agent_label();
CREATE TRIGGER aa_require_agent_label BEFORE INSERT OR UPDATE OR DELETE ON item_viewers
    FOR EACH STATEMENT EXECUTE FUNCTION pl_require_agent_label();
CREATE TRIGGER aa_require_agent_label BEFORE INSERT OR UPDATE OR DELETE ON item_reminders
    FOR EACH STATEMENT EXECUTE FUNCTION pl_require_agent_label();
CREATE TRIGGER aa_require_agent_label BEFORE INSERT OR UPDATE OR DELETE ON item_occurrences
    FOR EACH STATEMENT EXECUTE FUNCTION pl_require_agent_label();
CREATE TRIGGER aa_require_agent_label BEFORE INSERT OR UPDATE OR DELETE ON member_settings
    FOR EACH STATEMENT EXECUTE FUNCTION pl_require_agent_label();

RESET search_path;
RESET ROLE;
