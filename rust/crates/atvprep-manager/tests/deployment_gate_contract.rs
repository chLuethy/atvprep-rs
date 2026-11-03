//! Operator text follows the Python Unicode and character-count contract.
use atvprep_manager::{
    clock::FixedClock,
    db::Db,
    deployment_gate::{DeploymentGate, Operator, valid_operator_text},
    repo::Ctx,
    schema,
};
use serde_json::Value;
#[test]
fn operator_notes_match_recorded_python_validation() {
    for case in atvprep_testkit::load_golden::<String, Value>("deployment_gate") {
        let valid = case.expected["valid"].as_bool().unwrap();
        assert_eq!(valid_operator_text(&case.input), valid, "{}", case.name);
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        let clock = FixedClock(1_700_000_000_000_000);
        let gate = DeploymentGate(Ctx {
            conn: conn.raw(),
            clock: &clock,
        });
        let operator = Operator::from_admin_session(1).unwrap();
        let result = gate.close(&operator, &case.input);
        assert_eq!(result.is_ok(), valid, "{}", case.name);
        if valid {
            conn.raw().execute("INSERT INTO deployment_permits(kind,holder,token,generation) VALUES('job','legacy','legacy',1)",[]).unwrap();
            gate.abandon_permit(1, &operator, &case.input).unwrap();
        }
        if !valid {
            assert_eq!(
                result.unwrap_err().to_string(),
                case.expected["error"].as_str().unwrap()
            );
            let error = gate
                .abandon_permit(999, &operator, &case.input)
                .unwrap_err();
            assert_eq!(
                error.to_string(),
                case.expected["error"]
                    .as_str()
                    .unwrap()
                    .replace("reason", "evidence")
            );
        }
    }
}
