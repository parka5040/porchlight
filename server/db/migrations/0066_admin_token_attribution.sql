SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- Issuance (including local recovery) records only public metadata and the transaction actor.
-- No token or hash enters the outbox; this administrative kind is not delivered to device streams.
CREATE FUNCTION pl_token_created() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, porchlight, pg_temp
    AS $$
BEGIN
    INSERT INTO outbox(household_id,kind,entity_type,entity_id,payload)
    VALUES(NEW.household_id,'token.created','api_token',NEW.id,
        jsonb_build_object('actor',pl_current_actor(),'owner_member_id',NEW.owner_member_id,
                           'expires_at',NEW.expires_at));
    PERFORM pg_notify('porchlight_outbox','');
    RETURN NULL;
END $$;
CREATE TRIGGER api_tokens_created AFTER INSERT ON api_tokens
    FOR EACH ROW EXECUTE FUNCTION pl_token_created();

RESET search_path;
RESET ROLE;
