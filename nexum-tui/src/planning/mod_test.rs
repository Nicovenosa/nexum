//! Tests de flag, elegibilidad y clase de tarea del PlanningGateway.

use super::*;

#[test]
fn test_flag_off_por_defecto() {
    let _guard = crate::hormiguero::bridge::test_env_lock();
    std::env::remove_var("NEXUM_PLANNING");
    std::env::remove_var("NEXUM_PUBLIC_DEMO");
    assert!(!planning_enabled(), "planning OFF por defecto");
    std::env::set_var("NEXUM_PLANNING", "on");
    assert!(planning_enabled(), "on explícito activa");
    std::env::set_var("NEXUM_PUBLIC_DEMO", "1");
    assert!(!planning_enabled(), "public demo fuerza OFF");
    std::env::remove_var("NEXUM_PUBLIC_DEMO");
    std::env::remove_var("NEXUM_PLANNING");
}

#[test]
fn test_elegibilidad_tareas_planificables() {
    assert!(is_planning_eligible("escribí una función que ordene una lista"));
    assert!(is_planning_eligible("implementá el parser y agregá tests"));
    assert!(is_planning_eligible("primero crear el archivo, luego modificarlo"));
    assert!(is_planning_eligible("analizá el repo y buscá vulnerabilidades"));
}

#[test]
fn test_no_elegibles_triviales() {
    assert!(!is_planning_eligible("hola"), "saludo no planificable");
    assert!(!is_planning_eligible("gracias"), "corto sin trigger");
    assert!(!is_planning_eligible("qué hora es"), "pregunta trivial sin trigger");
}

#[test]
fn test_task_class() {
    assert_eq!(task_class_for("escribí código en python"), "code");
    assert_eq!(task_class_for("arreglá el bug del parser"), "code");
    assert_eq!(task_class_for("analizá el mercado de acciones"), "generic");
}
