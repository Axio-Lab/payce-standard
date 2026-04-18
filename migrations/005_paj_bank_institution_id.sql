-- PAJ /pub/offramp expects `bank` = institution id (same as SaveBank bankId), not saved-account id.
ALTER TABLE user_paj_bank_accounts
    ADD COLUMN IF NOT EXISTS paj_bank_institution_id TEXT;
