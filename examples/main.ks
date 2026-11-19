proc main() {
    // prints
    print("Meu nome e Edynu\n");
    print("Estou aprendendo Kestrel\n");

    // declaração de variaveis
    variavel1: int = 5;
    printf("exemplo de %%t = %t = 5\n", variavel1);
    printf("exemplo de %%v = %v = 5\n", variavel1);
    variavel1 = -5;
    printf("\n%i\n", variavel1);

    // operações
    variavel2: int = 2;
    printl(variavel2 + 2);
    printl(variavel2 - 2);
    printl(variavel2 * 2);
    printl(variavel2 / 2);

    variavel3: int = variavel2 + variavel1;
    printl(variavel3);
    variavel3: int = variavel2 - variavel1;
    printl(variavel3);
    variavel3: int = variavel2 * variavel1;
    printl(variavel3);
    variavel3: real = (real) variavel2 / (real) variavel1;
    printl(variavel3);
}
