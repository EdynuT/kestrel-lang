proc main() {
    // prints
    print("Meu nome e Edynu\n");
    print("Estou aprendendo Kestrel\n");

    // declaração de variaveis
    variavel1: int = 5;
    show(variavel1);
    print("\n");
    variavel1 = -5;
    show(variavel1);
    print("\n");

    // operações
    variavel2: int = 2;
    show(variavel2 + 2);
    print("\n");
    show(variavel2 - 2);
    print("\n");
    show(variavel2 * 2);
    print("\n");
    show(variavel2 / 2);
    print("\n");

    variavel3: int = variavel2 + variavel1;
    show(variavel3);
    print("\n");
    variavel3: int = variavel2 - variavel1;
    show(variavel3);
    print("\n");
    variavel3: int = variavel2 * variavel1;
    show(variavel3);
    print("\n");
    variavel3: real = (real) variavel2 / (real) variavel1;
    show(variavel3);
    print("\n");
}
